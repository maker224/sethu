# Changelog

All notable changes to Sethu are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/).

## [0.4.0] — 2026-11-10

### Added

- **Bidirectional file sharing** is now explicit: the Files tab works from the
  MacBook too, with a **Share** action on every file.
- **Send to Android** pushes a shared file directly into the phone or tablet's
  Downloads folder over adb (`POST /api/files/{id}/send`).
- **Copy download link** for Windows, Android or any browser that cannot be
  pushed to directly.
- File tests now cover the adb push path with the fake-adb harness.

### Changed

- Files copy makes the direction clear: upload from this device, download on
  any other.

## [0.3.0] — 2026-11-10

### Added

- **Files workspace**: upload files of any format from any browser (Android,
  Windows, iPhone, iPad, Mac) and download them on any other device, with
  drag-and-drop, multiple uploads, progress, size display and deletion.
- Streaming uploads with a configurable cap (`SETHU_MAX_FILE_BYTES`), safe
  filename handling and attachment-only downloads.
- **Open-access mode** (`SETHU_AUTH=open`) that removes the sign-in screen and
  shares one workspace between everyone who can reach the server.
- Per-workspace file metadata in `sethu.json` with contents under
  `data/files/`.
- Integration tests for multipart upload/download/delete, body-size overrides,
  size limits, cleanup and open-access mode.

### Changed

- The dashboard has **Devices** and **Files** workspace tabs alongside the
  platform filters.
- API JSON body limits now apply per router so large uploads stream without
  weakening normal JSON endpoints.

## [0.2.0] — 2026-11-10

### Added

- **Windows PCs** as first-class targets through any standard VNC server
  (TightVNC, UltraVNC, RealVNC), reusing the RFB bridge.
- **Android phones and tablets** through `adb`: PNG screenshot streaming at
  3–12 fps, tap and swipe gestures, text input, home/back/recents/power buttons
  and screenshot download.
- **Platform tabs** on the dashboard (All / Mac / Windows / iPhone-iPad /
  Android) with a four-way device picker and per-platform setup cards.
- Shared `viewer-stream.js` engine for iPhone and Android frame viewers,
  including pixel-to-point scaling and a single ordered frame drain loop.
- Configuration for Android: `SETHU_ANDROID_FPS`, `SETHU_ADB_PATH`,
  `SETHU_ADB_TIMEOUT_SECONDS`.
- Fake-`adb` integration test covering screenshot streaming, input injection and
  the reachability endpoint.

### Changed

- Device names, icons, cards and documentation now cover desktops and phones
  generically rather than Apple-only.
- Windows VNC targets share the macOS viewer and credential dialog.

### Known limitations

- Windows requires a third-party VNC server; RDP is not proxied.
- Android uses screenshot polling: no audio, DRM video or high frame rates.
- No built-in TLS; deploy behind Caddy/nginx/Traefik.

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
[0.2.0]: https://github.com/your-org/sethu/releases/tag/v0.2.0
[0.3.0]: https://github.com/your-org/sethu/releases/tag/v0.3.0
[0.4.0]: https://github.com/your-org/sethu/releases/tag/v0.4.0
