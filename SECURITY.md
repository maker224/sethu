# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Use GitHub's
private vulnerability reporting ("Report a vulnerability" on the Security tab)
or email the maintainer listed in the repository profile. Include:

- affected version or commit,
- reproduction steps or a proof of concept,
- impact assessment,
- any suggested fix.

You can expect an acknowledgement within 72 hours. We will coordinate a fix and
a disclosure date with you.

## Threat model

Sethu is a remote-access gateway. It is designed for **trusted operators on
trusted networks**, with TLS and strong credentials in front. Both device
side-channels (WDA and adb) are powerful and unauthenticated; Sethu is the only
authentication layer.

### In scope

- Authentication bypass, session fixation, CSRF, XSS or SQL/JSON injection.
- Path traversal or content-type confusion through file uploads/downloads.
- Cross-origin access to the API or WebSockets.
- Leakage of session tokens, password hashes, files or device addresses through
  logs, errors or the data file.
- Resource exhaustion from unauthenticated requests (within reason).
- Denial of service that survives rate limits, body-size caps and upload limits.

### Out of scope / accepted risk

- **A compromised server can control connected devices.** That is the product.
- **VNC's authentication is weak by design.** The legacy RFB password is capped
  at 8 characters and uses an old challenge-response exchange. macOS account
  auth (ARD) is stronger, but the Sethu process is still the TLS endpoint and
  could impersonate the host. Windows VNC servers typically only support the
  legacy password. Use a VPN, a strong password and firewall rules.
- **WebDriverAgent has no authentication.** Port 8100 grants full device control.
  Keep it on loopback, a private LAN or a VPN.
- **adb grants shell access to the Android device.** Anyone with access to the
  adb server can run commands as the `shell` user. Keep the Sethu host and the
  adb server trusted; never expose port 5037 or wireless-debugging ports.
- **Plaintext HTTP deployments** without a TLS proxy. Sethu logs a warning, but
  it cannot protect credentials on the wire by itself.
- **Open access is a deliberate downgrade.** `SETHU_AUTH=open` removes
  authentication and CSRF; everyone who reaches the port controls the first
  account's devices and files. Only run it on a trusted LAN/VPN or behind an
  authenticating proxy.
- **File sharing is not end-to-end encrypted.** Uploads are stored in plaintext
  under `data/files/` (honouring the data directory permissions) and are visible
  to everyone with access to the workspace. Sethu does not scan for malware.
  Downloads are forced attachments with `nosniff` to prevent execution on the
  Sethu origin.
- Accounts are not isolated from each other's *devices* beyond ownership checks;
  there are no admin roles in v0.3.
- Traffic analysis and screen contents are visible to the server operator.

## Hardening checklist

- [ ] Terminate TLS with Caddy/nginx/Traefik and set `SETHU_PUBLIC_URL` to the
      exact public origin.
- [ ] `SETHU_COOKIE_SECURE=true`.
- [ ] Set `SETHU_TRUST_PROXY=true` only when a local reverse proxy overwrites
      `X-Forwarded-*`.
- [ ] Create the owner account, then `SETHU_ALLOW_REGISTRATION=false`.
- [ ] Prefer `SETHU_AUTH=required`; if using `SETHU_AUTH=open`, put the server on
      a VPN or trusted LAN and never port-forward it.
- [ ] Set a sensible `SETHU_MAX_FILE_BYTES`, monitor disk space, and delete files
      you no longer need.
- [ ] Expose Sethu over a VPN or an IP allow-list, not the open internet.
- [ ] Never publish ports 5900, 8100, 5555 or the adb server (5037).
- [ ] Use a long, unique VNC password.
- [ ] Keep the data directory on an encrypted volume, mode `0700`, file `0600`.
- [ ] Back up `sethu.json` securely; it contains password hashes and session
      hashes.
- [ ] Run as an unprivileged user (the Docker image and systemd unit do).
- [ ] Watch logs for repeated `login rate limit hit` or `rejected cross-origin`
      messages.
- [ ] Install adb from a trusted platform-tools package and keep wireless
      debugging off when not in use.

## Security design notes

**Passwords.** Argon2id with the `argon2` crate defaults (19 MiB, t=2, p=1),
random 16-byte salts, verified in constant time. Unknown-email logins run a
dummy verify so timing does not enumerate accounts.

**Sessions.** 32 random bytes from the OS, hex encoded. The database stores only
SHA-256(token). Cookies are `HttpOnly; SameSite=Strict`, `Secure` when
configured, scoped to `/`. Logout deletes the record server-side.

**CSRF.** Each session has an independent 24-byte CSRF token, exposed only via
`GET /api/session` to same-origin JavaScript and required in
`x-sethu-csrf` for `POST`/`PATCH`/`DELETE` on `/api`. Login and registration are
exempt from CSRF (no session yet) but rate limited instead.

**Origin checks.** Browsers attach `Origin` to WebSocket upgrades and cross-site
fetches. The guard rejects any `Origin` whose host does not match the request
`Host` (accounting for `X-Forwarded-Host` when `SETHU_TRUST_PROXY` is enabled),
`SETHU_PUBLIC_URL`, or an entry in `SETHU_ALLOWED_ORIGINS`. Non-browser clients
without `Origin` still need a valid session cookie and CSRF token.

**Input handling.** Device targets are parsed and canonicalised
(`host:port` for VNC, `http(s)://host[:port][/prefix]` for WDA, a strict serial
for adb). Credentials in URLs, queries and fragments are rejected. WebSocket
text commands are parsed with serde and coordinate-checked. Android text input
is restricted to a safe character allowlist before it reaches
`adb shell input text`. Uploaded filenames are reduced to a sanitised basename,
stored under a server-generated UUID, and served as attachments. API bodies are
capped at 64 KiB; uploads stream with a configurable cap; WebSocket frames are
capped at 16 MiB.

**Headers.** `Content-Security-Policy` denies inline scripts (noVNC is loaded as
a module), frames, objects and form posts to other origins. `X-Frame-Options:
DENY`, `nosniff`, `Referrer-Policy: no-referrer`, restrictive
`Permissions-Policy`, `Cross-Origin-Opener-Policy: same-origin`. HSTS is emitted
when `SETHU_COOKIE_SECURE=true`.

**Storage.** Atomic replace of a mode-0600 JSON file. Session hashes and Argon2
hashes only — no plaintext device passwords. Still treat the file as sensitive.
