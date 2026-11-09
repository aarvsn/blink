//! `/api/v1/servers` — server lifecycle management (panel-facing).

use crate::docker;
use crate::error::{AppError, AppResult};
use crate::models::{states, CargoFile, CreateServerRequest, ServerConfig, UpdateServerRequest};
use crate::util;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::path::Path as StdPath;
use std::sync::Arc;

/// Broadcast an install log line to every connected console.
fn broadcast_install_log(state: &AppState, server_id: &str, message: &str, is_error: bool) {
    state.console_hub.broadcast_install_log(server_id, message, is_error);
}

/// Install logger mirroring the original's `InstallLogger` (ring buffer + console).
struct InstallLogger<'a> {
    state: &'a AppState,
    server_id: String,
    buffer: Vec<String>,
}

impl<'a> InstallLogger<'a> {
    fn new(state: &'a AppState, server_id: &str) -> Self {
        InstallLogger { state, server_id: server_id.to_string(), buffer: Vec::new() }
    }

    fn log(&mut self, message: &str) {
        let timestamp = chrono_now_iso();
        self.buffer.push(format!("[{timestamp}] {message}"));
        tracing::info!(server = %self.server_id, "{message}");
        broadcast_install_log(self.state, &self.server_id, message, false);
    }

    fn log_error(&mut self, message: &str) {
        let timestamp = chrono_now_iso();
        self.buffer.push(format!("[{timestamp}] {message}"));
        tracing::error!(server = %self.server_id, "{message}");
        broadcast_install_log(self.state, &self.server_id, message, true);
    }

    fn write_failure_log(&self, volume_path: &StdPath) {
        let log_path = volume_path.join("installation.log");
        let _ = std::fs::write(log_path, self.buffer.join("\n"));
    }
}

fn chrono_now_iso() -> String {
    // Lightweight ISO-8601 UTC timestamp without a chrono dependency.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // Days since epoch to Y-M-D conversion (Howard Hinnant's algorithm).
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Write panel-provided config files into `dir/config/<path>`.
async fn write_config_files(
    install_path: &StdPath,
    config: &ServerConfig,
    logger: &mut InstallLogger<'_>,
) -> AppResult<()> {
    for file in &config.config_files {
        let safe_path = util::safe_join(&install_path.join("config"), &file.path)
            .ok_or_else(|| AppError::bad_request(format!("invalid config file path: {}", file.path)))?;
        if let Some(parent) = safe_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&safe_path, &file.content).await.map_err(|e| {
            AppError::internal(format!("failed to write config file {}: {e}", file.path))
        })?;
        logger.log(&format!("Created config file: {}", file.path));
    }
    Ok(())
}

/// Download and materialise cargo files into a directory.
async fn process_cargo_files(
    panel: &crate::PanelClient,
    target_root: &StdPath,
    cargo: &[CargoFile],
    state: &AppState,
    server_id: &str,
    logger: &mut InstallLogger<'_>,
) -> AppResult<()> {
    for file in cargo {
        let safe_path = util::safe_join(target_root, &file.target_path)
            .ok_or_else(|| AppError::bad_request(format!("invalid cargo target path: {}", file.target_path)))?;
        if let Some(parent) = safe_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        logger.log(&format!("Downloading cargo file to {}...", file.target_path));
        panel.download_to(&file.url, &safe_path).await.map_err(|e| {
            logger.log_error(&format!("Failed to process cargo file {}", file.target_path));
            e
        })?;

        if file.properties.is_readonly() {
            let mut perms = std::fs::metadata(&safe_path)?.permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o444);
            std::fs::set_permissions(&safe_path, perms)?;
        }

        logger.log(&format!("Successfully processed cargo file: {}", file.target_path));
        let _ = state;
        let _ = server_id;
    }
    Ok(())
}

/// The full install pipeline: workspace -> config files -> images -> script -> container.
async fn run_install(
    state: &Arc<AppState>,
    server_id: &str,
    config: ServerConfig,
    memory_limit: i64,
) -> AppResult<()> {
    let safe_server_id = util::sanitize_volume_name(server_id);
    let volume_path = state.config.volume_path(&safe_server_id);
    let install_path = volume_path.join(".installation");

    let mut logger = InstallLogger::new(state, server_id);

    let result: AppResult<()> = async {
        tokio::fs::create_dir_all(install_path.join("logs")).await?;
        tokio::fs::create_dir_all(install_path.join("temp")).await?;
        logger.log("Creating installation environment...");

        write_config_files(&install_path, &config, &mut logger).await?;

        if let Some(cargo) = &config.cargo {
            if !cargo.is_empty() {
                logger.log("Processing cargo files...");
                process_cargo_files(&state.panel, &install_path, cargo, state, server_id, &mut logger)
                    .await?;
            }
        }

        let install = config
            .install
            .clone()
            .ok_or_else(|| AppError::bad_request("server config has no install script"))?;

        logger.log("Pulling required Docker images...");
        for image in [&install.docker_image, &config.docker_image] {
            logger.log(&format!("Pulling image: {image}"));
            docker::pull_image(&state.docker, image).await.map_err(|e| {
                logger.log_error(&format!("Failed to pull {image}"));
                e
            })?;
            logger.log(&format!("Successfully pulled {image}"));
        }

        logger.log("Preparing installation script...");
        let mut script = String::from(
            r#"#!/bin/bash
set -e
exec 1> >(tee -a "/mnt/server/.installation/logs/install.log")
exec 2>&1

echo "=== Installation Started at $(date) ==="
echo "Setting up environment..."

# Setup error handling
trap 'echo "Error on line $LINENO" >> /mnt/server/.installation/logs/install.log' ERR

# Process variables
"#,
        );
        for v in &config.variables {
            let value = v.current_value.clone().unwrap_or_else(|| v.default_value.clone());
            script.push_str(&format!("export {}=\"{}\"\n", v.name, value));
        }
        script.push_str("\n# Main installation script\n");
        script.push_str("echo \"Running main installation script...\"\n");
        script.push_str(&install.script);
        script.push_str("\n\nEXIT_CODE=$?\n\necho \"=== Installation Finished at $(date) with exit code $EXIT_CODE ===\"\nexit $EXIT_CODE\n");

        let script_path = install_path.join("install.sh");
        tokio::fs::write(&script_path, &script).await?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms)?;
        }

        logger.log("Starting installation container...");
        let container_id = docker::create_install_container(
            &state.docker,
            &safe_server_id,
            &install.docker_image,
            &config.variables,
            memory_limit,
            volume_path.to_string_lossy().as_ref(),
        )
        .await?;

        // Stream install logs to the console while the container runs.
        let docker = state.docker.clone();
        let log_container_id = container_id.clone();
        let log_server_id = server_id.to_string();
        let log_task = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut logs = docker::follow_logs(&docker, &log_container_id);
            while let Some(item) = logs.next().await {
                if let Ok(output) = item {
                    let msg = output.to_string().trim().to_string();
                    if !msg.is_empty() {
                        tracing::info!(server = %log_server_id, "{msg}");
                    }
                }
            }
        });

        state.docker.start_container::<String>(&container_id, None).await?;
        let exit_code = docker::wait_container(&state.docker, &container_id).await;
        log_task.abort();

        match exit_code {
            Ok(0) => {}
            Ok(code) => {
                let msg = format!("Installation failed with exit code {code}");
                logger.log_error(&msg);
                return Err(AppError::BadGateway(msg));
            }
            Err(e) => {
                logger.log_error(&format!("Failed waiting for install container: {e}"));
                return Err(e);
            }
        }

        let _ = docker::remove_container(&state.docker, &container_id, true, false).await;

        logger.log("Installation completed successfully");
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            let _ = tokio::fs::remove_dir_all(&install_path).await;
            Ok(())
        }
        Err(e) => {
            logger.log_error(&format!("Installation failed: {e}"));
            logger.write_failure_log(&volume_path);
            let _ = tokio::fs::remove_dir_all(&install_path).await;
            Err(e)
        }
    }
}

/// POST /api/v1/servers
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateServerRequest>,
) -> Response {
    let server_id = body.server_id.clone();

    let server_config = match state.panel.fetch_server_config(&server_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to create server {server_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
                .into_response();
        }
    };

    let startup = body.startup_command.clone().unwrap_or_else(|| server_config.startup_command.clone());
    let image = body.docker_image.clone().unwrap_or_else(|| server_config.docker_image.clone());

    if let Err(e) = state.db.insert_server(
        &server_id,
        &body.name,
        states::INSTALLING,
        body.memory_limit,
        body.cpu_limit,
        &image,
        &serde_json::to_string(&server_config.variables).unwrap_or_else(|_| "[]".into()),
        &startup,
        &serde_json::to_string(&server_config.install).unwrap_or_else(|_| "null".into()),
        &serde_json::to_string(&body.allocation).unwrap_or_else(|_| "{}".into()),
    ) {
        tracing::error!("failed to persist server {server_id}: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response();
    }

    // Confirm identity back to the panel before kicking off installation.
    let response = (
        StatusCode::CREATED,
        Json(json!({
            "id": server_id,
            "name": body.name,
            "state": states::INSTALLING,
            "validationToken": body.validation_token
        })),
    )
        .into_response();

    let install_state = state.clone();
    let install_server_id = server_id.clone();
    let install_config = server_config.clone();
    let memory_limit = body.memory_limit;
    let cpu_limit = body.cpu_limit;
    let allocation = body.allocation.clone();

    tokio::spawn(async move {
        let install_result =
            run_install(&install_state, &install_server_id, install_config.clone(), memory_limit).await;

        match install_result {
            Ok(()) => {
                let safe = util::sanitize_volume_name(&install_server_id);
                let volume = install_state.config.volume_path(&safe);
                let startup = install_config.startup_command.clone();
                let processed = util::process_variables(&startup, &install_config.variables, install_config.cargo.as_deref())
                    .unwrap_or(startup);

                match docker::create_game_container(
                    &install_state.docker,
                    &safe,
                    &install_config.docker_image,
                    &processed,
                    &install_config.variables,
                    memory_limit,
                    cpu_limit,
                    &allocation,
                    volume.to_string_lossy().as_ref(),
                )
                .await
                {
                    Ok(container_id) => {
                        let _ = install_state
                            .db
                            .update_docker_id_and_state(&install_server_id, Some(&container_id), states::INSTALLED);
                        install_state.console_hub.broadcast_install_log(
                            &install_server_id,
                            "[blinkd Daemon] Server installation completed successfully!",
                            false,
                        );
                    }
                    Err(e) => {
                        tracing::error!("container creation failed for {install_server_id}: {e}");
                        let _ = install_state
                            .db
                            .update_state(&install_server_id, states::INSTALL_FAILED);
                    }
                }
            }
            Err(e) => {
                tracing::error!("installation failed for {install_server_id}: {e}");
                let _ = install_state.db.update_state(&install_server_id, states::INSTALL_FAILED);
            }
        }
    });

    response
}

/// PATCH /api/v1/servers/:id — recreate the container with new settings.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(update_data): Json<UpdateServerRequest>,
) -> Response {
    if update_data.server_id != id {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid server ID in request data" })))
            .into_response();
    }

    let server = match state.db.get_server(&id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response()
        }
        Err(e) => return e.into_response(),
    };

    let _ = state.db.update_state(&id, states::UPDATING);

    let result: AppResult<String> = async {
        // Resolve the new configuration.
        let (image, startup, variables): (String, String, Vec<crate::models::ServerVariable>) =
            if update_data.unit_changed {
                let config = state.panel.fetch_server_config(&id).await?;
                let image = update_data
                    .docker_image
                    .clone()
                    .unwrap_or_else(|| config.docker_image.clone());
                (image, config.startup_command.clone(), config.variables.clone())
            } else {
                let image = update_data
                    .docker_image
                    .clone()
                    .unwrap_or_else(|| server.image.clone());
                let vars: Vec<crate::models::ServerVariable> =
                    serde_json::from_str(&server.variables).unwrap_or_default();
                (image, server.startup_command.clone(), vars)
            };

        let processed = util::process_variables(&startup, &variables, None).unwrap_or(startup);

        // Pull a new image when explicitly changed.
        if update_data.unit_changed {
            if let Some(image) = &update_data.docker_image {
                docker::pull_image(&state.docker, image).await?;
            }
        }

        // Stop and remove the old container, keeping the volume.
        if let Some(docker_id) = &server.docker_id {
            let running = docker::container_state(&state.docker, docker_id)
                .await
                .map(|s| s == "running")
                .unwrap_or(false);
            if running {
                let _ = docker::stop_container(&state.docker, docker_id, 10).await;
            }
            let _ = docker::remove_container(&state.docker, docker_id, true, false).await;
            tracing::info!(server = %id, "removed old container for update");
        }

        let allocation = update_data
            .allocation
            .clone()
            .or_else(|| serde_json::from_str(&server.allocation).ok())
            .ok_or_else(|| AppError::internal("server has no stored allocation"))?;

        let safe = util::sanitize_volume_name(&id);
        let volume = state.config.volume_path(&safe);
        let memory_limit = update_data.memory_limit.unwrap_or(server.memory_limit);
        let cpu_limit = update_data.cpu_limit.unwrap_or(server.cpu_limit);
        let name = update_data.name.clone().unwrap_or_else(|| server.name.clone());

        let container_id = docker::create_game_container(
            &state.docker,
            &safe,
            &image,
            &processed,
            &variables,
            memory_limit,
            cpu_limit,
            &allocation,
            volume.to_string_lossy().as_ref(),
        )
        .await?;

        state.docker.start_container::<String>(&container_id, None).await?;

        state.db.update_server_full(&id, Some(&container_id), states::RUNNING, &name, memory_limit, cpu_limit, &image)?;
        Ok(container_id)
    }
    .await;

    match result {
        Ok(container_id) => {
            let server = state.db.get_server(&id).ok().flatten();
            Json(json!({
                "message": "Server updated successfully",
                "server": server,
                "containerId": container_id
            }))
            .into_response()
        }
        Err(e) => {
            let _ = state.db.update_state(&id, states::UPDATE_FAILED);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

/// GET /api/v1/servers
pub async fn list(State(state): State<Arc<AppState>>) -> Response {
    match state.db.list_servers() {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/servers/:id
pub async fn get_one(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.db.get_server(&id) {
        Ok(Some(mut row)) => {
            if let Some(docker_id) = row.docker_id.clone() {
                row.status = Some(docker::container_status_json(&state.docker, &docker_id).await);
            } else {
                row.status = Some(json!({ "state": "unknown" }));
            }
            Json(row).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/v1/servers/:id
pub async fn remove(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let server = match state.db.get_server(&id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response()
        }
        Err(e) => return e.into_response(),
    };

    let _ = state.db.update_state(&id, states::DELETING);

    if let Some(docker_id) = &server.docker_id {
        if let Err(e) = docker::remove_container(&state.docker, docker_id, true, true).await {
            tracing::warn!(server = %id, "failed to remove container: {e}");
        }
    }

    let safe_id = util::sanitize_volume_name(&id);
    let volume_path = state.config.volume_path(&safe_id);
    if let Err(e) = tokio::fs::remove_dir_all(&volume_path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(server = %id, "failed to remove volume: {e}");
        }
    }

    match state.db.delete_server(&id) {
        Ok(true) => Json(json!({ "message": "Server deleted successfully" })).into_response(),
        Ok(false) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/servers/:id/cargo/ship
pub async fn cargo_ship(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let cargo: Vec<CargoFile> = match body.get("cargo").and_then(|c| serde_json::from_value(c.clone()).ok()) {
        Some(c) => c,
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid cargo format" })))
                .into_response()
        }
    };

    if !state.db.server_exists(&id).unwrap_or(false) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response();
    }

    let safe_id = util::sanitize_volume_name(&id);
    let volume_path = state.config.volume_path(&safe_id);

    let mut logger = InstallLogger::new(&state, &id);
    match process_cargo_files(&state.panel, &volume_path, &cargo, &state, &id, &mut logger).await {
        Ok(()) => Json(json!({ "message": "Cargo shipped successfully" })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response(),
    }
}

/// POST /api/v1/servers/:id/reinstall
pub async fn reinstall(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let server = match state.db.get_server(&id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Server not found" }))).into_response()
        }
        Err(e) => return e.into_response(),
    };

    let server_config = match state.panel.fetch_server_config(&id).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    };

    if let Some(docker_id) = &server.docker_id {
        let _ = docker::remove_container(&state.docker, docker_id, true, false).await;
        tracing::info!(server = %id, "removed existing container for reinstall");
    }

    let _ = state.db.update_docker_id_and_state(&id, None, states::INSTALLING);

    let install_state = state.clone();
    let install_id = id.clone();
    tokio::spawn(async move {
        match run_install(&install_state, &install_id, server_config, server.memory_limit).await {
            Ok(()) => {
                let _ = install_state.db.update_state(&install_id, states::INSTALLED);
                tracing::info!(server = %install_id, "reinstallation completed");
            }
            Err(e) => {
                tracing::error!(server = %install_id, "reinstallation failed: {e}");
                let _ = install_state.db.update_state(&install_id, states::INSTALL_FAILED);
            }
        }
    });

    // The original waits for completion; the panel timeout is 10s, installs take
    // longer, so accept the request and run it in the background (state updates
    // flow back through the DB and console broadcast).
    Json(json!({ "message": "Reinstallation started" })).into_response()
}

/// POST /api/v1/servers/:id/power/:action
pub async fn power(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Response {
    if !["start", "stop", "restart"].contains(&action.as_str()) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "Invalid power action" }))).into_response();
    }

    let docker_id = match state.db.get_docker_id(&id) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Server or container not found" })),
            )
                .into_response()
        }
        Err(e) => return e.into_response(),
    };

    let result = match action.as_str() {
        "start" => state
            .docker
            .start_container::<String>(&docker_id, None)
            .await
            .map_err(AppError::from),
        "stop" => docker::stop_container(&state.docker, &docker_id, 30).await.map(|_| ()),
        "restart" => docker::restart_container(&state.docker, &docker_id, 30).await.map(|_| ()),
        _ => unreachable!(),
    };

    if let Err(e) = result {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response();
    }

    // Sync the daemon state with reality.
    let new_state = docker::container_state(&state.docker, &docker_id).await.unwrap_or_default();
    let _ = state.db.update_state(&id, &new_state);

    Json(json!({ "message": format!("Power action {action} completed successfully") })).into_response()
}

