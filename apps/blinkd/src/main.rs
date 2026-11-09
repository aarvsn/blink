//! blinkd — the Blink node daemon.
//!
//! Runs on every game server node. Talks to the Blink panel over HTTP and
//! manages game servers as Docker containers. Exposes:
//!
//! * `POST/GET/PATCH/DELETE /api/v1/servers`  — server lifecycle (panel, API key)
//! * `POST /api/v1/servers/:id/power/:action` — power control (panel, API key)
//! * `POST /api/v1/servers/:id/cargo/ship`    — cargo file shipping (panel, API key)
//! * `GET  /api/v1/state`                     — node health (panel, API key)
//! * `GET/POST/DELETE /api/v1/filesystem/...` — file manager (browser, panel-validated token)
//! * `GET  /?server=..&token=..` (WebSocket)  — live console (browser, panel-validated token)

mod auth;
mod config;
mod db;
mod docker;
mod error;
mod models;
mod panel;
mod routes;
mod util;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub use config::Config;
pub use db::Db;
pub use panel::PanelClient;

/// Shared application state.
pub struct AppState {
    pub docker: bollard::Docker,
    pub db: Db,
    pub config: Config,
    pub panel: PanelClient,
    pub console_hub: ws::ConsoleHub,
    pub validation_cache: panel::ValidationCache,
    pub state_cache: tokio::sync::Mutex<Option<(routes::state::SystemState, std::time::Instant)>>,
    /// Per-IP live WebSocket console connections (enforced limit).
    pub ip_connections: tokio::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blinkd=info,tower_http=warn".into()),
        )
        .init();

    let cli = std::env::args().nth(1);
    match cli.as_deref() {
        Some("configure") => config::configure_wizard(),
        Some("version") | Some("--version") | Some("-V") => {
            println!("blinkd {VERSION}");
        }
        Some("--help") | Some("-h") | Some("help") => print_help(),
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            print_help();
            std::process::exit(2);
        }
        None => run().await,
    }
}

fn print_help() {
    println!(
        "blinkd {VERSION} — Blink node daemon

USAGE:
    blinkd [command]

COMMANDS:
    (none)       Start the daemon (config is read from blinkd.yml)
    configure    Interactive configuration wizard
    version      Print version"
    );
}

async fn run() {
    let config_path = std::env::var("BLINKD_CONFIG").unwrap_or_else(|_| "blinkd.yml".to_string());
    let config = match config::Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to load config from {config_path}: {e}");
            std::process::exit(1);
        }
    };

    if config.api_key == "your-secret-key" {
        tracing::warn!(
            "apiKey is the well-known default. Generate one with `blinkd configure` \
             or `openssl rand -hex 32` before exposing this node."
        );
    }

    // Volumes directory must exist before containers bind-mount it.
    if let Err(e) = tokio::fs::create_dir_all(&config.volumes_directory).await {
        tracing::error!("failed to create volumes directory {}: {e}", config.volumes_directory);
        std::process::exit(1);
    }

    // Connect to Docker. The client construction fails when no daemon socket
    // exists at all; that is fatal because every container operation would be
    // a guaranteed failure.
    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("could not connect to the Docker daemon: {e}");
            tracing::error!("blinkd requires Docker. Start Docker and try again.");
            std::process::exit(1);
        }
    };

    // A present-but-stopped daemon is tolerated: container endpoints will
    // return errors per-request, filesystem/console validation keeps working.
    match docker.ping().await {
        Ok(_) => tracing::info!("docker daemon connected"),
        Err(e) => tracing::warn!(
            "Docker daemon is not reachable ({e}). Container operations will fail \
             until Docker is available."
        ),
    }

    let db = match Db::open("blinkd.db") {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("failed to open blinkd.db: {e}");
            std::process::exit(1);
        }
    };

    let panel = PanelClient::new(config.app_url.clone(), config.api_key.clone());
    let validation_cache = panel::ValidationCache::new(config.validation_cache_ttl_secs);

    let state = Arc::new(AppState {
        docker: docker.clone(),
        db,
        config,
        panel,
        console_hub: ws::ConsoleHub::new(),
        validation_cache,
        state_cache: tokio::sync::Mutex::new(None),
        ip_connections: tokio::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let app = build_router(state.clone());

    let addr: SocketAddr = format!("{}:{}", state.config.bind_address, state.config.bind_port)
        .parse()
        .expect("invalid bind address/port");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    tracing::info!("blinkd {VERSION} listening on {}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("server error");
}

fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::list(
            state
                .config
                .cors_origins()
                .into_iter()
                .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok()),
        ))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::HeaderName::from_static("x-api-key"),
        ])
        .allow_credentials(true);

    let max_upload = state.config.max_upload_size_mb * 1024 * 1024;

    // Panel-facing routes: authenticated with the node API key.
    let servers = Router::new()
        .route("/", post(routes::servers::create).get(routes::servers::list))
        .route(
            "/{id}",
            axum::routing::patch(routes::servers::update)
                .get(routes::servers::get_one)
                .delete(routes::servers::remove),
        )
        .route("/{id}/reinstall", post(routes::servers::reinstall))
        .route("/{id}/power/{action}", post(routes::servers::power))
        .route("/{id}/cargo/ship", post(routes::servers::cargo_ship))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024));

    let state_route = Router::new()
        .route("/", get(routes::state::get_state))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ));

    // Browser-facing file manager: token validated against the panel.
    let filesystem = Router::new()
        .route("/{server_id}/list/{*path}", get(routes::filesystem::list))
        .route("/{server_id}/list/", get(routes::filesystem::list_root))
        .route("/{server_id}/contents/{*path}", get(routes::filesystem::contents))
        .route("/{server_id}/write/{*path}", post(routes::filesystem::write))
        .route("/{server_id}/delete/{*path}", delete(routes::filesystem::delete_path))
        .route("/{server_id}/create-directory/{*path}", post(routes::filesystem::create_directory))
        .route("/{server_id}/rename", post(routes::filesystem::rename))
        .route("/{server_id}/copy", post(routes::filesystem::copy))
        .route("/{server_id}/chmod", post(routes::filesystem::chmod))
        .route("/{server_id}/upload/{*path}", post(routes::filesystem::upload))
        .route("/{server_id}/upload/", post(routes::filesystem::upload_root))
        .route("/{server_id}/download/{*path}", get(routes::filesystem::download))
        .route("/{server_id}/download/", get(routes::filesystem::download_root))
        .route("/{server_id}/extract/{*path}", post(routes::filesystem::extract))
        .route("/{server_id}/disk-usage", get(routes::filesystem::disk_usage))
        .route("/{server_id}/search", get(routes::filesystem::search))
        .route("/{server_id}/compress", post(routes::filesystem::compress))
        .route("/{server_id}/cargo-info/{*path}", get(routes::filesystem::cargo_info))
        .route("/{server_id}/cargo-files", get(routes::filesystem::cargo_files))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_panel_token,
        ))
        .layer(DefaultBodyLimit::max(max_upload))
        .layer(RequestBodyLimitLayer::new(max_upload));

    Router::new()
        .nest("/api/v1/servers", servers)
        .nest("/api/v1/state", state_route)
        .nest("/api/v1/filesystem", filesystem)
        .route("/", get(ws::console::ws_handler))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
