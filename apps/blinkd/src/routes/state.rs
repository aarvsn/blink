//! `/api/v1/state` — node health information (panel-facing, API-key protected).

use crate::error::AppResult;
use crate::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Must stay JSON-compatible with what the panel's node view renders.
#[derive(Debug, Clone, Serialize)]
pub struct SystemState {
    pub version: String,
    pub kernel: String,
    pub os_version: String,
    pub hostname: String,
    pub cpu_cores: usize,
    pub memory_total: u64,
    pub containers: ContainerStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerStats {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
}

pub async fn get_state(State(state): State<Arc<AppState>>) -> AppResult<Json<SystemState>> {
    {
        let cache = state.state_cache.lock().await;
        if let Some((cached, at)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(Json(cached.clone()));
            }
        }
    }

    let sys = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new()
            .with_memory(sysinfo::MemoryRefreshKind::new().with_ram())
            .with_cpu(sysinfo::CpuRefreshKind::new()),
    );
    let hostname = sysinfo::System::host_name().unwrap_or_default();
    let kernel = sysinfo::System::kernel_version().unwrap_or_default();
    let os_version = sysinfo::System::long_os_version().unwrap_or_default();
    let cpu_cores = sys.physical_core_count().unwrap_or_else(|| sys.cpus().len());
    let memory_total = sys.total_memory();

    let containers = collect_container_stats(&state.docker).await;

    let state_payload = SystemState {
        version: crate::VERSION.to_string(),
        kernel,
        os_version,
        hostname,
        cpu_cores,
        memory_total,
        containers,
    };

    let mut cache = state.state_cache.lock().await;
    *cache = Some((state_payload.clone(), Instant::now()));

    Ok(Json(state_payload))
}

async fn collect_container_stats(docker: &bollard::Docker) -> ContainerStats {
    let options = bollard::container::ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    };

    match docker.list_containers(Some(options)).await {
        Ok(list) => {
            let total = list.len();
            let running = list
                .iter()
                .filter(|c| c.state.as_deref() == Some("running"))
                .count();
            ContainerStats {
                total,
                running,
                stopped: total.saturating_sub(running),
            }
        }
        Err(e) => {
            tracing::warn!("failed to list containers for state endpoint: {e}");
            ContainerStats { total: 0, running: 0, stopped: 0 }
        }
    }
}
