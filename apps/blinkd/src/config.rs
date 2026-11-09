//! Daemon configuration (`blinkd.yml`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Shared secret the panel uses to authenticate against this node.
    pub api_key: String,
    /// Address the HTTP/WebSocket server binds to.
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// Port the HTTP/WebSocket server binds to.
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    /// Directory that holds per-server data volumes.
    #[serde(default = "default_volumes_directory")]
    pub volumes_directory: String,
    /// Base URL of the Blink panel (used to fetch server configs and validate tokens).
    pub app_url: String,
    /// Comma-separated list of allowed CORS origins.
    #[serde(default = "default_cors_origin")]
    pub cors_origin: String,
    /// How long a panel token validation result may be cached (seconds).
    #[serde(default = "default_validation_cache_ttl")]
    pub validation_cache_ttl_secs: u64,
    /// Maximum concurrent console WebSockets per client IP.
    #[serde(default = "default_max_connections_per_ip")]
    pub max_connections_per_ip: usize,
    /// Maximum accepted file upload size (megabytes).
    #[serde(default = "default_max_upload_size_mb")]
    pub max_upload_size_mb: usize,
}

fn default_bind_address() -> String {
    "127.0.0.1".into()
}
fn default_bind_port() -> u16 {
    8080
}
fn default_volumes_directory() -> String {
    "./volumes".into()
}
fn default_cors_origin() -> String {
    "http://localhost:5173".into()
}
fn default_validation_cache_ttl() -> u64 {
    60
}
fn default_max_connections_per_ip() -> usize {
    10
}
fn default_max_upload_size_mb() -> usize {
    100
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api_key: "your-secret-key".into(),
            bind_address: default_bind_address(),
            bind_port: default_bind_port(),
            volumes_directory: default_volumes_directory(),
            app_url: "http://localhost:3000".into(),
            cors_origin: default_cors_origin(),
            validation_cache_ttl_secs: default_validation_cache_ttl(),
            max_connections_per_ip: default_max_connections_per_ip(),
            max_upload_size_mb: default_max_upload_size_mb(),
        }
    }
}

impl Config {
    /// Load the YAML config, writing a template first if the file is missing.
    pub fn load(path: &str) -> Result<Config, String> {
        let path = Path::new(path);
        if !path.exists() {
            let template = serde_yaml::to_string(&Config::default())
                .map_err(|e| format!("template serialisation failed: {e}"))?;
            std::fs::write(path, template)
                .map_err(|e| format!("could not write template {}: {e}", path.display()))?;
            tracing::warn!(
                "no config found — wrote a template to {}. Edit it (especially apiKey and appUrl) and restart.",
                path.display()
            );
            return Ok(Config::default());
        }

        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let mut config: Config =
            serde_yaml::from_str(&raw).map_err(|e| format!("invalid YAML in {}: {e}", path.display()))?;

        config.app_url = config.app_url.trim_end_matches('/').to_string();
        if config.app_url.is_empty() {
            return Err("app_url must not be empty".into());
        }
        if config.api_key.is_empty() {
            return Err("api_key must not be empty".into());
        }
        if config.max_upload_size_mb == 0 {
            config.max_upload_size_mb = default_max_upload_size_mb();
        }
        Ok(config)
    }

    /// Parsed CORS allowlist.
    pub fn cors_origins(&self) -> Vec<String> {
        self.cors_origin
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Absolute path of a server volume.
    pub fn volume_path(&self, safe_server_id: &str) -> PathBuf {
        Path::new(&self.volumes_directory).join(safe_server_id)
    }
}

/// Interactive wizard that writes `blinkd.yml` (mirrors the panel `configure` flow).
pub fn configure_wizard() {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut out = std::io::stdout();

    let mut ask = |prompt: &str, default: &str| -> String {
        print!("{prompt} [{default}]: ");
        let _ = out.flush();
        match lines.next() {
            Some(Ok(line)) if !line.trim().is_empty() => line.trim().to_string(),
            _ => default.to_string(),
        }
    };

    println!("blinkd configuration wizard");
    println!("This will write blinkd.yml in the current directory.\n");

    let existing = Config::load("blinkd.yml").unwrap_or_default();

    let panel_url = ask("Panel URL", &existing.app_url);
    let port = ask("Port to bind", &existing.bind_port.to_string());
    let key = ask(
        "Connection key (must match the node's key in the panel)",
        &existing.api_key,
    );
    let origins = ask("Allowed CORS origins (comma-separated)", &existing.cors_origin);

    let config = Config {
        api_key: key,
        bind_address: "0.0.0.0".into(),
        bind_port: port.parse().unwrap_or(default_bind_port()),
        volumes_directory: existing.volumes_directory.clone(),
        app_url: panel_url.trim_end_matches('/').to_string(),
        cors_origin: origins,
        ..existing
    };

    let yaml = match serde_yaml::to_string(&config) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("failed to serialise config: {e}");
            std::process::exit(1);
        }
    };

    let banner = "# Generated by `blinkd configure`\n# Restart blinkd after changing this file.\n\n";
    if let Err(e) = std::fs::write("blinkd.yml", format!("{banner}{yaml}")) {
        eprintln!("failed to write blinkd.yml: {e}");
        std::process::exit(1);
    }

    println!("\nWrote blinkd.yml. Start the daemon with `blinkd`.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let yaml = r#"
apiKey: abc123
bindAddress: 0.0.0.0
bindPort: 8081
volumesDirectory: /var/lib/blinkd/volumes
appUrl: https://panel.example.com/
corsOrigin: https://panel.example.com,https://www.example.com
validationCacheTtlSecs: 120
maxConnectionsPerIp: 5
maxUploadSizeMb: 250
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.bind_port, 8081);
        assert_eq!(c.validation_cache_ttl_secs, 120);
        assert_eq!(c.max_upload_size_mb, 250);
        assert_eq!(
            c.cors_origins(),
            vec!["https://panel.example.com".to_string(), "https://www.example.com".to_string()]
        );
    }

    #[test]
    fn defaults_apply_for_missing_fields() {
        let c: Config = serde_yaml::from_str("apiKey: k\nappUrl: http://localhost:3000").unwrap();
        assert_eq!(c.bind_port, 8080);
        assert_eq!(c.bind_address, "127.0.0.1");
        assert_eq!(c.max_connections_per_ip, 10);
    }
}
