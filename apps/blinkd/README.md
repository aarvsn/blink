# blinkd

The Blink node daemon — Rust implementation of the daemon that runs on every
machine hosting game servers. It manages one Docker container per server,
serves the file-manager API and streams live consoles over WebSocket.

```bash
cargo build --release
./target/release/blinkd            # writes a template blinkd.yml on first run
$EDITOR blinkd.yml                 # set apiKey + appUrl
./target/release/blinkd
```

Commands: `blinkd`, `blinkd configure`, `blinkd version`.

See the full documentation in `apps/docs/content/docs/`:
`blinkd-configuration.mdx` and `blinkd-security.mdx`.

## Development

```bash
cargo test        # unit tests (path safety, config, variables, db)
cargo clippy      # lints — CI runs with -D warnings
```

Wire contract: the panel (apps/panel) talks to this daemon over HTTP with
`X-API-Key`; browsers use per-server validation tokens validated against the
panel. Keep both sides in sync when changing routes or events.
