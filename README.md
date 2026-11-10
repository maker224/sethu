# Sethu

**Your Mac and iPhone, on any screen.**

Sethu is a self-hosted, MIT-licensed remote-access gateway written in Rust. It
puts your Apple devices inside a browser tab so you can use them from Windows,
Android, a Chromebook, Linux or an iPad — without installing a client or
sending traffic through someone else's cloud.

```
Windows / Android / iPad / Linux
        │  HTTPS + WSS
        ▼
  ┌───────────────────┐
  │   Sethu (Rust)    │  accounts · sessions · CSRF · rate limits
  └─────────┬─────────┘
            │
    ┌───────┴────────────────────┐
    │                            │
    ▼ RFB (VNC)                  ▼ HTTP screenshots + WDA commands
macOS Screen Sharing :5900    WebDriverAgent :8100 ──► iPhone / iPad
```

| Device | What you get | Transport |
| --- | --- | --- |
| **Mac / MacBook** | Full desktop, keyboard and mouse, clipboard, scaling, view-only mode | RFB over a WebSocket-to-TCP bridge (noVNC in the browser) |
| **iPhone / iPad** | Live screen, tap, swipe, text input, home/lock buttons, screenshot download, 4–15 fps | PNG frames and WDA commands over WebSocket |

Everything runs on one small binary. Accounts, device list and sessions live in
a single JSON file on your server. Device passwords are never stored or sent to
the Sethu backend.

![Sethu dashboard with two Macs and an iPhone](docs/dashboard.png)

> **Scope, honestly:** macOS is reached through its built-in Screen Sharing
> server, so Screen Sharing must be enabled and allowed for your user (account
> auth) or protected with a VNC password. iPhone mirroring uses WebDriverAgent,
> which cannot capture audio, DRM-protected video or the lock screen. See
> [Limitations](#limitations).

---

## Quick start

Requirements: Rust 1.82+ and a Mac on the same network (for Screen Sharing), or
a Mac with WebDriverAgent for iPhone mirroring.

```bash
git clone https://github.com/your-org/sethu.git
cd sethu
cargo run --release
```

Open <http://localhost:8080> and create the first account. Then add a device.

During development, serve the frontend from disk so edits are instant:

```bash
SETHU_WEB_DIR=./web RUST_LOG=sethu_server=debug cargo run
```

### Docker

```bash
cp .env.example .env       # set SETHU_PUBLIC_URL / secrets
docker compose up -d --build
```

Data persists in the `sethu-data` volume. Put a TLS-terminating proxy in front
before exposing it, and read [Security](SECURITY.md) first.

---

## Connect a Mac

1. On the Mac: **System Settings → General → Sharing → Screen Sharing** on.
2. Choose how Sethu should sign in:
   - **Your macOS account (recommended).** Under *Allow access for*, add your
     user. Sethu prompts for your Mac username and password using Apple Remote
     Desktop authentication.
   - **A VNC password.** Click the ⓘ → **Computer Settings…** → enable
     **VNC viewers may control screen with password**. This avoids typing your
     account password into a browser, but the VNC secret is limited to 8
     characters.
3. Allow Screen Sharing through the macOS firewall. Verify from the Sethu host:

   ```bash
   nc -vz studio-mac.local 5900
   ```

4. In Sethu, **Add device → Mac**, enter `studio-mac.local` (port defaults to
   5900). **Test** should report a TCP handshake.
5. **Connect** and sign in. Sethu never stores the credentials; they travel from
   the browser to the Mac through the encrypted WebSocket.

`scripts/enable-macos-screen-sharing.sh` automates the sharing toggle (it uses
Apple's `kickstart` tool and asks for `sudo`); it does not guess or store the
VNC password.

---

## Connect an iPhone or iPad

Sethu talks to [WebDriverAgent][wda] (the XCTest runner used by Appium). No
Appium server is required — Sethu calls WDA's session-less endpoints directly.

1. On a Mac with Xcode: build and run WebDriverAgent for your device. The helper
   script clones and runs it:

   ```bash
   ./scripts/start-wda.sh <device-udid>
   # or set WDA_DIR / DEVELOPMENT_TEAM_ID and run it again
   ```

2. WDA listens on the device at port 8100. Expose it to the Sethu process with
   `iproxy` (or `go-ios`):

   ```bash
   iproxy 8100 8100 <device-udid>
   ```

   Easiest setup: run Sethu **on the same Mac** and add `http://127.0.0.1:8100`.
   On iOS 17+ XCTest communication needs the userspace tunnel; `go-ios` and
   `pymobiledevice3` provide it.

3. Verify: `curl http://127.0.0.1:8100/status` returns `"ready": true`.

4. In Sethu, **Add device → iPhone / iPad**, enter the base URL, for example
   `http://192.168.1.20:8100`.

> **WDA has no authentication.** Anyone who can reach port 8100 controls the
> phone. Keep it on loopback, a private LAN or a VPN — never expose it to the
> internet. Sethu adds accounts and TLS on top, but WDA itself is the trust
> boundary.

Gesture mapping: a short press is a tap; a drag becomes a WDA
`dragfromtoforduration` gesture; **Type** sends text via `/wda/keys`. Screenshot
pixels are converted to logical points using the scale reported by
`/wda/screen`.

[wda]: https://github.com/appium/WebDriverAgent

---

## Configuration

All settings are environment variables (or an `.env` file for Docker Compose).
See `.env.example`.

| Variable | Default | Purpose |
| --- | --- | --- |
| `SETHU_BIND` | `0.0.0.0:8080` | Listen address |
| `SETHU_DATA_DIR` | `./data` | Directory for `sethu.json` (mode 0600) |
| `SETHU_PUBLIC_URL` | – | Canonical URL, e.g. `https://remote.example.com`; required behind a proxy |
| `SETHU_COOKIE_SECURE` | auto (`true` with an `https://` public URL) | Marks the session cookie `Secure` |
| `SETHU_ALLOW_REGISTRATION` | `true` | Set `false` after creating your account |
| `SETHU_SESSION_TTL_HOURS` | `168` | Login lifetime |
| `SETHU_IOS_FPS` | `10` | Default iPhone frame rate (1–15) |
| `SETHU_TRUST_PROXY` | `false` | Trust `X-Forwarded-For` / `X-Forwarded-Host` from a local proxy |
| `SETHU_ALLOWED_ORIGINS` | – | Extra comma-separated browser origins |
| `SETHU_WDA_TIMEOUT_SECONDS` | `6` | Per-request timeout for WebDriverAgent |
| `SETHU_VNC_CONNECT_TIMEOUT_SECONDS` | `10` | TCP connect timeout for VNC |
| `SETHU_MAX_SESSIONS` | `32` | Maximum concurrent VNC/iPhone streams |
| `SETHU_WEB_DIR` | – | Serve frontend files from disk (development) |
| `SETHU_VENDOR_DIR` | `./web/vendor` | Optional third-party assets (vendored noVNC) |
| `RUST_LOG` | `sethu_server=info` | `tracing` filter |

### Reverse proxy + TLS

Sethu speaks plain HTTP so a proxy can own certificates. Example Caddy config in
[`deploy/Caddyfile`](deploy/Caddyfile):

```
remote.example.com {
    encode zstd gzip
    reverse_proxy 127.0.0.1:8080 {
        flush_interval -1
    }
}
```

Then set:

```dotenv
SETHU_PUBLIC_URL=https://remote.example.com
SETHU_COOKIE_SECURE=true
SETHU_TRUST_PROXY=true
```

`flush_interval -1` matters for screen sharing: it disables proxy buffering so
WebSocket frames are not delayed.

### systemd

```bash
sudo useradd --system --home /var/lib/sethu sethu
sudo install -d -o sethu -g sethu /var/lib/sethu
sudo install -m 0644 deploy/sethu.service /etc/systemd/system/sethu.service
sudo install -m 0600 .env.example /etc/sethu/sethu.env   # edit first
sudo systemctl enable --now sethu
```

---

## HTTP API

The frontend is a static client of this JSON API. All browser mutations require
the `x-sethu-csrf` header returned by `GET /api/session`; the session cookie is
`HttpOnly; SameSite=Strict`.

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/api/health` | Liveness, version |
| `GET` | `/api/session` | Current user, CSRF token, registration flag |
| `POST` | `/api/auth/register` | Create account (`email`, `password`) |
| `POST` | `/api/auth/login` | Sign in |
| `POST` | `/api/auth/logout` | Revoke the session |
| `GET` | `/api/hosts` | List your devices |
| `POST` | `/api/hosts` | Add device (`name`, `kind`: `macos`\|`ios`, `target`, `notes`) |
| `GET` | `/api/hosts/{id}` | One device |
| `PATCH` | `/api/hosts/{id}` | Rename or edit notes |
| `DELETE` | `/api/hosts/{id}` | Remove device |
| `POST` | `/api/hosts/{id}/test` | TCP reachability (Mac) or WDA `/status` (iPhone) |
| `GET` (WS) | `/ws/vnc/{id}` | Raw RFB byte stream |
| `GET` (WS) | `/ws/ios/{id}` | PNG frames + JSON commands |

WebSocket details and the iPhone message schema are documented in
[`ARCHITECTURE.md`](ARCHITECTURE.md).

---

## Security model

- **Argon2id** password hashing, constant-time verification and equalised
  timing for unknown emails.
- **Sessions** are 256-bit random tokens; only SHA-256 hashes are persisted.
  Cookie is `HttpOnly`, `SameSite=Strict`, optionally `Secure`.
- **CSRF** double-submit tokens on every unsafe request; **Origin/Host** checks
  also protect WebSocket upgrades.
- **Rate limiting** per IP on login and registration.
- **No stored device credentials.** The VNC password or macOS account
  credentials go from the browser to the Mac through the encrypted WebSocket;
  Sethu never parses or stores them. As the TLS endpoint, the server process
  could in principle intercept the stream — run it on hardware you trust.
- **Hardened headers**: CSP, `X-Frame-Options: DENY`, `nosniff`, no-referrer,
  restrictive permissions policy.
- The data file is written atomically with mode `0600`; back it up and protect it.

Sethu is not a zero-trust product. A compromised Sethu server, or an account
with access to a device, can control that device. Deploy it on a trusted network
or behind a VPN, terminate TLS, disable registration after onboarding, and keep
WDA bound to loopback. Full threat model: [`SECURITY.md`](SECURITY.md).

---

## Development

```
crates/sethu-server/
  src/    main.rs        binary, config, graceful shutdown
    lib.rs         router assembly
    api.rs         JSON API (auth + devices)
    auth.rs        Argon2, cookies, CSRF, AuthUser extractor
    security.rs    origin checks, rate limits, hardening headers
    store.rs       JSON persistence (users, hosts, hashed sessions)
    target.rs      device address validation
    wda.rs         WebDriverAgent client
    ws/vnc.rs      RFB <-> WebSocket bridge
    ws/ios.rs      screenshot producer + command bridge
    assets.rs      compiled-in frontend
    web.rs         static serving
web/               vanilla ES modules + CSS, no build step
scripts/           macOS / WDA / noVNC helpers
deploy/            systemd + Caddy examples
```

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
SETHU_WEB_DIR=./web cargo run
```

The frontend has no bundler or npm dependency. noVNC is loaded from a vendored
copy when present, otherwise from jsDelivr. For an offline install:

```bash
./scripts/vendor-novnc.sh
```

Contributions are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md).

---

## Limitations

- **macOS auth:** Screen Sharing must be enabled. Both the macOS account (Apple
  Remote Desktop auth) and the legacy VNC password are supported; the VNC
  password is capped at 8 characters by the protocol.
- **iPhone capture:** WDA screenshots are polled, not a video stream. Expect
  4–15 fps, no audio, no DRM video, and no lock screen or passcode UI.
- **WDA setup:** needs Xcode and a signed runner. On iOS 17+ XCTest needs a
  userspace tunnel (`go-ios`/`pymobiledevice3`).
- **No built-in TLS:** use Caddy, nginx or Traefik (examples included).
- **Single-user devices:** devices belong to the account that added them; no
  sharing between accounts yet.
- **No file transfer, audio, multi-monitor selection or recording** in v0.1.

### Roadmap

- Native macOS agent (ScreenCaptureKit + H.264/WebRTC) for stronger auth and
  lower latency
- iPhone video streaming via an MJPEG/H.264 pipeline
- TOTP / WebAuthn second factor and audit log
- Per-device sharing with expiring links
- iOS 17+ tunnel helper built into Sethu
- Session recording and clipboard sync improvements

---

## License

MIT © Sethu contributors. noVNC (loaded client-side) is MPL-2.0; WebDriverAgent is
BSD-3-Clause. See [`LICENSE`](LICENSE).
