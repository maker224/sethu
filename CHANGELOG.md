# Changelog

All notable changes to Sethu are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-11-10

Initial public release.

### Added

- Self-hosted Rust web application (axum + tokio) with a single-binary
  deployment and embedded frontend assets.
- macOS screen sharing through macOS Screen Sharing (RFB/VNC): raw WebSocket↔TCP
  passthrough, noVNC viewer with Apple Remote Desktop account auth *and* legacy
  VNC-password auth, scaling, view-only mode, quality presets and clipboard
  support.
- iPhone/iPad mirroring through WebDriverAgent: PNG frame streaming at 4–15 fps,
  tap/swipe/text input, home/lock buttons, screenshot download and a live fps
  indicator.
- Account registration and login with Argon2id hashing, hashed server-side
  sessions, CSRF tokens and per-IP auth rate limiting.
- Device management: add, list, test reachability, rename notes and delete;
  devices are scoped to their owner.
- Same-origin and `Host` enforcement for API and WebSocket traffic, plus
  hardening headers (CSP, no-referrer, frame denial, permissions policy).
- JSON persistence with atomic writes and mode-0600 data files.
- Dockerfile, Compose file, systemd unit, Caddy example and helper scripts for
  macOS Screen Sharing, WebDriverAgent and offline noVNC vendoring.
- Unit and integration tests for auth, CSRF, origin checks, host CRUD, target
  parsing and iPhone protocol handling.

### Known limitations

- macOS requires Screen Sharing to be enabled. Account auth (ARD) and the legacy
  8-character VNC password are both supported.
- iOS capture is screenshot polling: no audio, DRM video or lock screen.
- No built-in TLS; deploy behind Caddy/nginx/Traefik.
- No device sharing between accounts, file transfer or recording yet.

[0.1.0]: https://github.com/your-org/sethu/releases/tag/v0.1.0
