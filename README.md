# Blink

**A self-hosted game server management panel with a Rust-based node daemon.**

</div>

Blink manages game servers across multiple machines. The web panel handles server management, files and settings, while `blinkd` runs on each node and manages game servers in isolated Docker containers.

## Stack

| Component | Technology |
|---|---|
| Frontend | React 18, Vite, Tailwind CSS |
| API | Bun, Express, SQLite |
| Daemon | Rust, Axum, Bollard |
| Documentation | Next.js, Fumadocs |

## Features

- Web-based server and node management.
- Live WebSocket consoles.
- File manager with Monaco editor.
- Docker-based server isolation.
- CPU and memory limits.
- Pterodactyl-style egg support and importer.
- Managed files with access restrictions.
- Rust daemon with authentication and security controls.

## Installation

```bash
git clone https://github.com/aarvsn/blink.git
cd blink
./install.sh
````

Optional installer flags:

- `--systemd`
- `--skip-daemon`
- `--skip-docs`

After installation:

- Panel: `http://<host>:3000`
- Configure the daemon in `apps/blinkd/blinkd.yml`.
- Add the node through Admin → Nodes using its connection key.

The first registered account becomes the administrator.

See the [documentation](apps/docs/content/docs/introduction.mdx) for setup and configuration.

### Development

```bash
bun install

bun dev:panel:api
bun dev:panel:web
bun dev:docs

cargo run --manifest-path apps/blinkd/Cargo.toml
```

### Tests and builds

```bash
cargo test --manifest-path apps/blinkd/Cargo.toml
cargo clippy --manifest-path apps/blinkd/Cargo.toml
bun --cwd apps/panel/frontend build
bun run build:docs
```

### Security

Report vulnerabilities through [SECURITY.md](SECURITY.md).

See the [daemon security guide](apps/docs/content/docs/blinkd-security.mdx) for its security model and configuration.

### License

GPL-3.0

### Attribution

Blink is a fork of [Argon](https://github.com/argon-foss?utm_source=chatgpt.com), which is derived from Pterodactyl Panel and Skyport Labs. `blinkd` is a Rust reimplementation of Argon's Krypton daemon.
