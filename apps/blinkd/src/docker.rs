//! Docker container management via bollard 0.18 (all methods live on `Docker`).

use crate::error::{AppError, AppResult};
use crate::models::ServerVariable;
use bollard::container::{
    AttachContainerOptions, Config, CreateContainerOptions, KillContainerOptions, LogOutput,
    LogsOptions, RemoveContainerOptions, RestartContainerOptions, StatsOptions,
    StopContainerOptions, WaitContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum};
use bollard::Docker;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;

/// The panel sends CPU limits in Pterodactyl "shares" (1024 == 100% of one core).
/// Convert to Docker CpuQuota microseconds given CpuPeriod = 100_000.
pub fn cpu_quota_micros(cpu_limit: i64) -> i64 {
    if cpu_limit <= 0 {
        return 0;
    }
    cpu_limit.saturating_mul(100_000) / 1024
}

pub async fn pull_image(docker: &Docker, image: &str) -> AppResult<()> {
    tracing::info!("pulling Docker image: {image}");
    let options = CreateImageOptions::<String> {
        from_image: image.to_string(),
        ..Default::default()
    };

    let mut stream = docker.create_image(Some(options), None, None);
    let mut last_error = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(info) => {
                if let Some(status) = &info.status {
                    tracing::debug!("pull {image}: {status}");
                }
            }
            Err(e) => {
                last_error = e.to_string();
            }
        }
    }

    // create_image can end without an explicit error even for missing images;
    // verify by inspecting the image.
    match docker.inspect_image(image).await {
        Ok(_) => Ok(()),
        Err(_) => Err(AppError::BadGateway(format!(
            "failed to pull Docker image {image}: {last_error}"
        ))),
    }
}

fn port_bindings_map(bind_address: &str, port: u16) -> bollard::models::PortMap {
    let binding = vec![PortBinding {
        host_ip: Some(bind_address.to_string()),
        host_port: Some(port.to_string()),
    }];
    let mut map = HashMap::new();
    map.insert(format!("{port}/tcp"), Some(binding.clone()));
    map.insert(format!("{port}/udp"), Some(binding));
    map
}

fn exposed_ports(port: u16) -> HashMap<String, HashMap<(), ()>> {
    let mut map = HashMap::new();
    map.insert(format!("{port}/tcp"), HashMap::new());
    map.insert(format!("{port}/udp"), HashMap::new());
    map
}

fn game_host_config(
    volume_path: &str,
    memory_limit: i64,
    cpu_limit: i64,
    allocation: &crate::models::Allocation,
) -> HostConfig {
    HostConfig {
        memory: Some(memory_limit),
        memory_swap: Some(memory_limit.saturating_mul(2)),
        cpu_quota: Some(cpu_quota_micros(cpu_limit)),
        cpu_period: Some(100_000),
        network_mode: Some("bridge".into()),
        init: Some(true),
        security_opt: Some(vec!["no-new-privileges".into()]),
        readonly_paths: Some(vec![
            "/proc/bus".into(),
            "/proc/fs".into(),
            "/proc/irq".into(),
            "/proc/sys".into(),
            "/proc/sysrq-trigger".into(),
        ]),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),
        binds: Some(vec![format!("{volume_path}:/home/container:rw")]),
        port_bindings: Some(port_bindings_map(&allocation.bind_address, allocation.port)),
        ..Default::default()
    }
}

fn game_environment(startup_command: &str, variables: &[ServerVariable]) -> Vec<String> {
    let mut env = vec![
        "TERM=xterm".to_string(),
        "HOME=/home/container".to_string(),
        "USER=container".to_string(),
        format!("STARTUP={startup_command}"),
    ];
    for v in variables {
        let value = v
            .current_value
            .clone()
            .unwrap_or_else(|| v.default_value.clone());
        env.push(format!("{}={}", v.name, value));
    }
    env
}

/// Create (but do not start) the long-lived game container.
#[allow(clippy::too_many_arguments)]
pub async fn create_game_container(
    docker: &Docker,
    safe_server_id: &str,
    image: &str,
    startup_command: &str,
    variables: &[ServerVariable],
    memory_limit: i64,
    cpu_limit: i64,
    allocation: &crate::models::Allocation,
    volume_path: &str,
) -> AppResult<String> {
    let config = Config {
        image: Some(image.to_string()),
        working_dir: Some("/home/container".into()),
        user: Some("container".into()),
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        open_stdin: Some(true),
        tty: Some(false),
        stdin_once: Some(false),
        env: Some(game_environment(startup_command, variables)),
        exposed_ports: Some(exposed_ports(allocation.port)),
        host_config: Some(game_host_config(
            volume_path,
            memory_limit,
            cpu_limit,
            allocation,
        )),
        labels: Some(HashMap::from([
            ("blink.server.id".to_string(), safe_server_id.to_string()),
            ("blink.server.name".to_string(), safe_server_id.to_string()),
        ])),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: safe_server_id.to_string(),
        platform: None,
    };

    let created = docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to create container: {e}")))?;

    Ok(created.id)
}

/// Create the one-shot installation container (not started).
pub async fn create_install_container(
    docker: &Docker,
    safe_server_id: &str,
    install_image: &str,
    variables: &[ServerVariable],
    memory_limit: i64,
    volume_path: &str,
) -> AppResult<String> {
    let mut env = vec!["DEBIAN_FRONTEND=noninteractive".to_string()];
    for v in variables {
        let value = v
            .current_value
            .clone()
            .unwrap_or_else(|| v.default_value.clone());
        env.push(format!("{}={}", v.name, value));
    }

    let host_config = HostConfig {
        binds: Some(vec![format!("{volume_path}:/mnt/server:rw")]),
        memory: Some(memory_limit),
        memory_swap: Some(memory_limit.saturating_mul(2)),
        network_mode: Some("bridge".into()),
        auto_remove: Some(false),
        ..Default::default()
    };

    let config = Config {
        image: Some(install_image.to_string()),
        cmd: Some(vec!["bash".into(), ".installation/install.sh".into()]),
        working_dir: Some("/mnt/server".into()),
        env: Some(env),
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        open_stdin: Some(true),
        tty: Some(true),
        host_config: Some(host_config),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: format!("{safe_server_id}_install"),
        platform: None,
    };

    let created = docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to create install container: {e}")))?;

    Ok(created.id)
}

/// Attach to a container's stdin so a console command can be written to the
/// main process. Mirrors the original per-command `docker attach` behaviour.
pub async fn write_command_to_container(
    docker: &Docker,
    container_id: &str,
    command: &str,
) -> AppResult<()> {
    let options = AttachContainerOptions::<String> {
        detach_keys: None,
        stream: Some(false),
        stdin: Some(true),
        stdout: Some(false),
        stderr: Some(false),
        logs: Some(false),
    };

    let results = docker
        .attach_container(container_id, Some(options))
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to attach to container: {e}")))?;

    let mut input = results.input;
    use tokio::io::AsyncWriteExt;
    input
        .write_all(format!("{command}\n").as_bytes())
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to send command: {e}")))?;
    input.flush().await?;
    // Dropping input closes stdin for this attach client only.
    Ok(())
}

/// Follow a container's log stream (already demultiplexed by bollard).
pub fn follow_logs(
    docker: &Docker,
    container_id: &str,
) -> impl futures_util::Stream<Item = AppResult<LogOutput>> {
    let options = LogsOptions::<String> {
        follow: true,
        stdout: true,
        stderr: true,
        tail: "0".to_string(),
        timestamps: false,
        ..Default::default()
    };
    docker
        .logs(container_id, Some(options))
        .map(|item| item.map_err(AppError::from))
}

/// One-shot stats sample, mapped to the console `stats` payload.
pub async fn stats_sample(docker: &Docker, container_id: &str) -> AppResult<serde_json::Value> {
    let mut stream = docker.stats(container_id, Some(StatsOptions {
        stream: false,
        one_shot: false,
    }));

    let stats = match stream.next().await {
        Some(Ok(s)) => s,
        Some(Err(e)) => return Err(AppError::from(e)),
        None => return Err(AppError::internal("no stats returned")),
    };

    let state = container_state(docker, container_id)
        .await
        .unwrap_or_else(|| "unknown".to_string());

    let cpu_delta = stats
        .cpu_stats
        .cpu_usage
        .total_usage
        .saturating_sub(stats.precpu_stats.cpu_usage.total_usage);
    let system_delta = stats
        .cpu_stats
        .system_cpu_usage
        .unwrap_or(0)
        .saturating_sub(stats.precpu_stats.system_cpu_usage.unwrap_or(0));
    let cpu_count = stats.cpu_stats.online_cpus.unwrap_or(1).max(1) as f64;

    let cpu_percent = if system_delta > 0 && cpu_delta > 0 {
        ((cpu_delta as f64 / system_delta as f64) * cpu_count * 100.0).min(100.0)
    } else {
        0.0
    };

    let memory_used = stats.memory_stats.usage.unwrap_or(0);
    let memory_limit = stats.memory_stats.limit.unwrap_or(0);
    let memory_percent = if memory_limit > 0 {
        (memory_used as f64 / memory_limit as f64) * 100.0
    } else {
        0.0
    };

    let (rx, tx) = stats
        .networks
        .as_ref()
        .and_then(|n| n.get("eth0"))
        .map(|n| (n.rx_bytes, n.tx_bytes))
        .unwrap_or((0, 0));

    Ok(json!({
        "state": state,
        "cpu_percent": cpu_percent,
        "memory": {
            "used": memory_used,
            "limit": memory_limit,
            "percent": memory_percent
        },
        "network": {
            "rx_bytes": rx,
            "tx_bytes": tx
        }
    }))
}

/// Map Docker's status string to the daemon's state vocabulary.
pub fn normalise_state(status: &str) -> String {
    match status {
        "exited" => "stopped".to_string(),
        other => other.to_string(),
    }
}

/// Current container state string ("running", "stopped", ...).
pub async fn container_state(docker: &Docker, container_id: &str) -> Option<String> {
    let info = docker.inspect_container(container_id, None).await.ok()?;
    let status = info.state?.status?;
    Some(normalise_state(status.as_ref()))
}

/// Summary shown to the panel for GET /api/v1/servers/:id.
pub async fn container_status_json(docker: &Docker, container_id: &str) -> serde_json::Value {
    match docker.inspect_container(container_id, None).await {
        Ok(info) => {
            let state = info.state.unwrap_or_default();
            json!({
                "state": state.status.map(|s| s.to_string()).unwrap_or_else(|| "unknown".into()),
                "running": state.running.unwrap_or(false),
                "startedAt": state.started_at.unwrap_or_default(),
                "finishedAt": state.finished_at.unwrap_or_default()
            })
        }
        Err(_) => json!({ "state": "unknown" }),
    }
}

pub async fn stop_container(docker: &Docker, container_id: &str, timeout_secs: i64) -> AppResult<()> {
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: timeout_secs }))
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to stop container: {e}")))
}

pub async fn restart_container(docker: &Docker, container_id: &str, timeout_secs: i64) -> AppResult<()> {
    docker
        .restart_container(container_id, Some(RestartContainerOptions { t: timeout_secs as isize }))
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to restart container: {e}")))
}

pub async fn kill_container(docker: &Docker, container_id: &str) -> AppResult<()> {
    docker
        .kill_container(container_id, Some(KillContainerOptions { signal: "SIGKILL" }))
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to kill container: {e}")))
}

pub async fn remove_container(
    docker: &Docker,
    container_id: &str,
    force: bool,
    volumes: bool,
) -> AppResult<()> {
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                force,
                v: volumes,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| AppError::BadGateway(format!("failed to remove container: {e}")))
}

/// Wait until the container is not running; returns the exit code.
pub async fn wait_container(docker: &Docker, container_id: &str) -> AppResult<i64> {
    let options = WaitContainerOptions {
        condition: "not-running",
    };
    let mut stream = docker.wait_container(container_id, Some(options));
    match stream.next().await {
        Some(Ok(result)) => Ok(result.status_code),
        Some(Err(e)) => Err(AppError::BadGateway(format!("failed waiting for container: {e}"))),
        None => Err(AppError::BadGateway("container wait stream ended unexpectedly".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_quota_conversion() {
        // 1024 shares == 100% == 100000us quota for a 100000us period.
        assert_eq!(cpu_quota_micros(1024), 100_000);
        assert_eq!(cpu_quota_micros(512), 50_000);
        assert_eq!(cpu_quota_micros(0), 0);
        assert_eq!(cpu_quota_micros(-5), 0);
        // 100% CPU on the panel should never grant more than ~1 core.
        assert!(cpu_quota_micros(1024) < 200_000);
    }
}
