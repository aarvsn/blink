# Contributing to Blink

Thanks for your interest in improving Blink! This document covers the basics
of working in the monorepo.

## Getting set up

```bash
git clone https://github.com/aarvsn/blink.git
cd blink
bun install          # installs all JS workspaces
rustup component add clippy
```

## Where things live

| Path | What |
| ---- | ---- |
| `apps/panel/frontend` | React SPA (`@blink/panel-web`) |
| `apps/panel/server` | API server (`@blink/panel-api`), runs on Bun |
| `apps/blinkd` | Rust daemon (`blinkd`) |
| `apps/docs` | fumadocs documentation site |

## Rules of thumb

- **Keep the panel↔daemon contract stable.** The wire protocol between the
  panel and blinkd (HTTP routes, WebSocket events, DB states) is documented
  in `apps/docs/content/docs/`. If you change it, change both sides and the
  docs in the same PR.
- **Security-sensitive code needs tests.** Path handling, auth comparisons
  and variable templating in blinkd have unit tests — extend them when you
  touch that logic.
- **No secrets in logs.** Never log API keys, validation tokens or signed
  URLs.
- **One version per package.** The panel reads its version from
  `package.json`; blinkd from `Cargo.toml`. Don't hard-code versions.

## Before opening a PR

```bash
bun --cwd apps/panel/frontend build                          # typecheck + build
cargo clippy --manifest-path apps/blinkd/Cargo.toml -- -D warnings
cargo test --manifest-path apps/blinkd/Cargo.toml
cd apps/docs && bun run build                                # docs still build
```

CI runs the same checks. Commit messages should be short and descriptive
(`fix(blinkd): enforce per-IP websocket limit`, `docs: explain cargo
readonly flags`).

## Reporting bugs

Open a GitHub issue with: what you did, what you expected, what happened,
and the relevant log lines (`journalctl -u blinkd`, panel stdout). Redact
keys and tokens first.
