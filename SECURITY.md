# Security Policy

## Supported versions

Only the latest release of each component (panel, blinkd, docs) receives
security fixes. The panel and daemon are versioned and released together —
run matching versions on both sides.

## Reporting a vulnerability

Please report vulnerabilities privately via
[GitHub Security Advisories](https://github.com/aarvsn/blink/security/advisories/new)
rather than a public issue. Include:

- affected component and version (`blinkd version`, panel `GET /api/system`)
- steps to reproduce or a proof of concept
- impact assessment

You'll get an acknowledgement within a few days. Please allow reasonable
time for a fix and coordinated disclosure.

## Scope notes

- The **panel** executes unit install scripts on nodes **by design** (like
  Pterodactyl eggs). Anyone who can create units can run shell code on nodes
  — protect admin access accordingly.
- The **daemon** trusts the panel it is configured against (`appUrl`); point
  it at a panel you control.
- Reports about the upstream projects (Argon, Pterodactyl, Skyport) belong to
  their respective maintainers.

## Known design decisions

- Node connection keys are compared constant-time and never logged.
- Browser-facing filesystem/console endpoints validate tokens against the
  panel with a short TTL cache (`validationCacheTtlSecs`, default 60 s).
- `/api/v1/state` requires the node API key; the panel proxies it to keep
  hardware details off the public internet.
