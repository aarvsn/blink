//! Live console WebSocket.
//!
//! Protocol (JSON `{ event, data }`):
//! * client → daemon: `send_command`, `power_action`, `heartbeat`
//! * daemon → client: `console_output`, `stats`, `power_status`,
//!   `auth_success`, `daemon_message`, `error`, `heartbeat_ack`

use crate::auth;
use crate::docker;
use crate::models::{states, WsEvent};
use crate::util;
use crate::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode as AxumStatusCode;
use axum::response::IntoResponse;
use axum::extract::{ConnectInfo, RawQuery, State};
use axum::response::Response;
use futures_util::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};

const MAX_LOGS: usize = 100;
const INITIAL_LOGS: usize = 10;
const MAX_PAYLOAD: usize = 50 * 1024;
const STATS_INTERVAL_SECS: u64 = 2;

/// Shared console infrastructure: log ring buffers + install-log broadcast.
#[derive(Clone)]
pub struct ConsoleHub {
    inner: Arc<ConsoleHubInner>,
}

struct ConsoleHubInner {
    log_buffers: Mutex<HashMap<String, VecDeque<String>>>,
    install_tx: broadcast::Sender<String>,
}

impl ConsoleHub {
    pub fn new() -> Self {
        let (install_tx, _) = broadcast::channel(256);
        ConsoleHub {
            inner: Arc::new(ConsoleHubInner {
                log_buffers: Mutex::new(HashMap::new()),
                install_tx,
            }),
        }
    }

    pub async fn add_log(&self, internal_id: &str, log: &str) {
        let mut buffers = self.inner.log_buffers.lock().await;
        let buffer = buffers.entry(internal_id.to_string()).or_default();
        buffer.push_back(log.to_string());
        while buffer.len() > MAX_LOGS {
            buffer.pop_front();
        }
    }

    pub async fn last_logs(&self, internal_id: &str, count: usize) -> Vec<String> {
        let buffers = self.inner.log_buffers.lock().await;
        match buffers.get(internal_id) {
            Some(buffer) => buffer.iter().rev().take(count).rev().cloned().collect(),
            None => Vec::new(),
        }
    }

    pub async fn clear_logs(&self, internal_id: &str) {
        let mut buffers = self.inner.log_buffers.lock().await;
        buffers.remove(internal_id);
    }

    /// Broadcast install progress to every connected console (like the original).
    pub fn broadcast_install_log(&self, _server_id: &str, message: &str, is_error: bool) {
        let event = WsEvent::new(
            "console_output",
            serde_json::json!({
                "message": message,
                "timestamp": message_timestamp(),
                "type": if is_error { "error" } else { "info" }
            }),
        );
        let _ = self.inner.install_tx.send(serde_json::to_string(&event).unwrap_or_default());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.inner.install_tx.subscribe()
    }
}

fn message_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

#[derive(serde::Deserialize)]
struct ConsoleQuery {
    server: Option<String>,
    token: Option<String>,
}

pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let query: ConsoleQuery = raw_query
        .as_deref()
        .and_then(serde_urlencoded)
        .unwrap_or(ConsoleQuery { server: None, token: None });

    let Some(server_id) = query.server.map(|s| util::sanitize_server_id(&s)).filter(|s| !s.is_empty()) else {
        return close_reject(1008, "Missing server ID or token");
    };
    let Some(token) = query.token.filter(|t| !t.is_empty()) else {
        return close_reject(1008, "Missing server ID or token");
    };

    // Enforce the per-IP connection limit (the original had this disabled).
    {
        let mut connections = state.ip_connections.lock().await;
        let current = connections.entry(addr.ip()).or_insert(0);
        if *current >= state.config.max_connections_per_ip {
            return close_reject(1008, "Too many connections from this IP");
        }
        *current += 1;
    }

    // Validate the token with the panel (5s deadline, cached).
    let validation = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        auth::validate_cached(&state, &server_id, &token),
    )
    .await
    .ok()
    .flatten();

    match validation {
        Some(v) if v.validated => {}
        _ => {
            let mut connections = state.ip_connections.lock().await;
            if let Some(count) = connections.get_mut(&addr.ip()) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    connections.remove(&addr.ip());
                }
            }
            return close_reject(1008, "Invalid token or access denied");
        }
    }

    ws.on_upgrade(move |socket| handle_socket(state, socket, server_id, token, addr.ip()))
}

/// Minimal `application/x-www-form-urlencoded` parser (query strings only).
fn serde_urlencoded(query: &str) -> Option<ConsoleQuery> {
    let mut server = None;
    let mut token = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        let key = percent_encoding::percent_decode_str(k).decode_utf8_lossy().to_string();
        let value = percent_encoding::percent_decode_str(v).decode_utf8_lossy().to_string();
        match key.as_str() {
            "server" => server = Some(value),
            "token" => token = Some(value),
            _ => {}
        }
    }
    Some(ConsoleQuery { server, token })
}

fn close_reject(_code: u16, reason: &str) -> Response {
    (AxumStatusCode::UNAUTHORIZED, reason.to_string()).into_response()
}

struct ConnectionGuard {
    state: Arc<AppState>,
    ip: IpAddr,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let state = self.state.clone();
        let ip = self.ip;
        tokio::spawn(async move {
            let mut connections = state.ip_connections.lock().await;
            if let Some(count) = connections.get_mut(&ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    connections.remove(&ip);
                }
            }
        });
    }
}

async fn handle_socket(
    state: Arc<AppState>,
    mut socket: WebSocket,
    server_id: String,
    token: String,
    ip: IpAddr,
) {
    let _guard = ConnectionGuard { state: state.clone(), ip };

    // Look up the container.
    let docker_id = match state.db.get_docker_id(&server_id) {
        Ok(Some(id)) => id,
        _ => {
            tracing::error!(server = %server_id, "no container assigned for console session");
            let _ = socket
                .send(Message::Close(Some(CloseFrame { code: 1011, reason: "No container assigned".into() })))
                .await;
            return;
        }
    };

    let container_state = docker::container_state(&state.docker, &docker_id)
        .await
        .unwrap_or_else(|| "unknown".to_string());

    // Replay the tail of the log buffer.
    for log in state.console_hub.last_logs(&server_id, INITIAL_LOGS).await {
        let event = WsEvent::new("console_output", serde_json::json!({ "message": log }));
        let _ = socket.send(event.to_message()).await;
    }

    // Outbound channel: tasks (logs, stats, install broadcast) push here.
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Message>();

    // Initial stats snapshot.
    match docker::stats_sample(&state.docker, &docker_id).await {
        Ok(stats) => {
            let _ = outbound_tx.send(WsEvent::new("stats", stats).to_message());
        }
        Err(e) => {
            tracing::warn!(server = %server_id, "initial stats failed: {e}");
        }
    }

    let _ = outbound_tx
        .send(WsEvent::new("auth_success", serde_json::json!({ "state": container_state })).to_message());

    // Live log follower.
    spawn_log_follower(state.clone(), docker_id.clone(), server_id.clone(), outbound_tx.clone());

    // Periodic stats.
    spawn_stats_loop(state.clone(), docker_id.clone(), outbound_tx.clone());

    // Install progress broadcast.
    let mut install_rx = state.console_hub.subscribe();
    {
        let outbound_tx = outbound_tx.clone();
        tokio::spawn(async move {
            while let Ok(msg) = install_rx.recv().await {
                if outbound_tx.send(Message::Text(msg.into())).is_err() {
                    break;
                }
            }
        });
    }

    // Main loop: client messages and outbound fan-in.
    loop {
        tokio::select! {
            Some(outbound) = outbound_rx.recv() => {
                if socket.send(outbound).await.is_err() {
                    break;
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(err) = handle_client_message(
                            &state, &server_id, &docker_id, &token, &text, &outbound_tx,
                        ).await {
                            let _ = outbound_tx.send(
                                WsEvent::new("error", serde_json::json!({ "message": err })).to_message()
                            );
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = socket.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // binary/pong ignored
                    Some(Err(_)) => break,
                }
            }
        }
    }

    state.validation_cache.invalidate(&server_id, &token).await;
}

/// Spawn the background task that tails container logs into the console.
fn spawn_log_follower(state: Arc<AppState>, docker_id: String, server_id: String, outbound_tx: mpsc::UnboundedSender<Message>) {
    tokio::spawn(async move {
        let mut logs = docker::follow_logs(&state.docker, &docker_id);
        let mut line_buffer = String::new();
        while let Some(item) = logs.next().await {
            let Ok(output) = item else { break };
            let chunk = output.to_string();
            line_buffer.push_str(&chunk);

            // Emit complete lines, keep the trailing partial one.
            while let Some(idx) = line_buffer.find('\n') {
                let line: String = line_buffer.drain(..=idx).collect();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.len() > MAX_PAYLOAD {
                    continue;
                }
                state.console_hub.add_log(&server_id, line).await;
                let event = WsEvent::new("console_output", serde_json::json!({ "message": line }));
                if outbound_tx.send(event.to_message()).is_err() {
                    return;
                }
            }
        }
    });
}

/// Spawn the 2-second resource stats loop.
fn spawn_stats_loop(state: Arc<AppState>, docker_id: String, outbound_tx: mpsc::UnboundedSender<Message>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(STATS_INTERVAL_SECS));
        loop {
            interval.tick().await;
            match docker::stats_sample(&state.docker, &docker_id).await {
                Ok(stats) => {
                    if outbound_tx.send(WsEvent::new("stats", stats).to_message()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

async fn handle_client_message(
    state: &Arc<AppState>,
    server_id: &str,
    docker_id: &str,
    token: &str,
    text: &str,
    outbound_tx: &mpsc::UnboundedSender<Message>,
) -> Result<(), String> {
    if text.len() > MAX_PAYLOAD {
        return Err("Message exceeds maximum allowed size".into());
    }

    let parsed: serde_json::Value = serde_json::from_str(text).map_err(|_| "Invalid JSON".to_string())?;
    let event = parsed.get("event").and_then(|v| v.as_str()).unwrap_or_default().to_string();

    match event.as_str() {
        "send_command" => {
            let command = parsed
                .get("data")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            if command.is_empty() {
                return Ok(());
            }
            if command.len() > MAX_PAYLOAD {
                return Err("Command exceeds maximum allowed size".into());
            }

            // Keep printable ASCII but retain quotes (the command is written
            // straight to the container's stdin, never interpreted by a shell).
            let sanitized: String = command
                .chars()
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .collect();
            let sanitized = sanitized.trim();
            if sanitized.is_empty() {
                return Ok(());
            }

            docker::write_command_to_container(&state.docker, docker_id, sanitized)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }

        "power_action" => {
            let action = parsed
                .get("data")
                .and_then(|d| d.get("action"))
                .and_then(|a| a.as_str())
                .unwrap_or_default()
                .to_string();
            handle_power_action(state, server_id, docker_id, token, &action, outbound_tx).await
        }

        "heartbeat" => {
            let _ = outbound_tx.send(WsEvent::new("heartbeat_ack", serde_json::json!({})).to_message());
            Ok(())
        }

        other => Err(format!("unknown event: {other}")),
    }
}

async fn handle_power_action(
    state: &Arc<AppState>,
    server_id: &str,
    docker_id: &str,
    token: &str,
    action: &str,
    outbound_tx: &mpsc::UnboundedSender<Message>,
) -> Result<(), String> {
    if !["start", "stop", "restart", "kill"].contains(&action) {
        return Err("Invalid power action".into());
    }

    let daemon_prefix = "[blinkd Daemon]";
    send_console(state, server_id, &format!("{daemon_prefix} Performing a {action} action on server..."), outbound_tx);

    let current = docker::container_state(&state.docker, docker_id)
        .await
        .unwrap_or_else(|| "unknown".to_string());

    if (action == "start" && current == "running")
        || ((action == "stop" || action == "kill") && current == "stopped")
        || (action == "restart" && current == "restarting")
    {
        let _ = outbound_tx.send(
            WsEvent::new("error", serde_json::json!({ "message": format!("Server is already in {current} state") }))
                .to_message(),
        );
        return Ok(());
    }

    let result = match action {
        "start" => state
            .docker
            .start_container::<String>(docker_id, None)
            .await
            .map_err(crate::error::AppError::from),
        "stop" => docker::stop_container(&state.docker, docker_id, 30).await.map(|_| ()),
        "kill" => docker::kill_container(&state.docker, docker_id).await,
        "restart" => docker::restart_container(&state.docker, docker_id, 30).await.map(|_| ()),
        _ => unreachable!(),
    };

    if let Err(e) = result {
        let msg = format!("Failed to {action} server: {e}");
        send_console(state, server_id, &msg, outbound_tx);
        let _ = outbound_tx.send(WsEvent::new("error", serde_json::json!({ "message": e.to_string() })).to_message());
        return Ok(());
    }

    // Log buffers reset on power state changes (mirrors the original).
    state.console_hub.clear_logs(server_id).await;

    let new_state = docker::container_state(&state.docker, docker_id).await.unwrap_or_default();

    // Re-attach the log stream after a start or restart.
    if action == "start" || action == "restart" {
        spawn_log_follower(state.clone(), docker_id.to_string(), server_id.to_string(), outbound_tx.clone());
    }

    let status_message = if new_state == "running" {
        format!("{daemon_prefix} The server is now powered on and will begin the pre-boot process.")
    } else if action == "kill" {
        format!("{daemon_prefix} The server has been forcefully terminated.")
    } else {
        format!("{daemon_prefix} The server has successfully been powered off.")
    };

    let _ = outbound_tx
        .send(
            WsEvent::new(
                "power_status",
                serde_json::json!({
                    "status": status_message,
                    "action": action,
                    "state": new_state,
                    "error": ""
                }),
            )
            .to_message(),
        );

    // Keep the daemon-side DB state roughly in sync for panel queries.
    let db_state = match new_state.as_str() {
        "running" => states::RUNNING,
        "stopped" => states::STOPPED,
        _ => new_state.as_str(),
    };
    let _ = state.db.update_state(server_id, db_state);

    // A power action invalidates the cached validation so revoked tokens
    // take effect immediately on the next file/console operation.
    state.validation_cache.invalidate(server_id, token).await;

    Ok(())
}

fn send_console(state: &Arc<AppState>, server_id: &str, message: &str, outbound_tx: &mpsc::UnboundedSender<Message>) {
    let hub = state.console_hub.clone();
    let server = server_id.to_string();
    let msg = message.to_string();
    tokio::spawn(async move {
        hub.add_log(&server, &msg).await;
    });
    let event = WsEvent::new("console_output", serde_json::json!({ "message": message }));
    let _ = outbound_tx.send(event.to_message());
}
