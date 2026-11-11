# Architecture

Sethu is deliberately small: one Rust binary serves the API, the frontend and
two WebSocket bridges. There is no database, message broker or worker process.

## Components

```
┌──────────────────────────────────────────────────────────────────────┐
│ Browser (vanilla ES modules, no build step)                          │
│  index.html + app.js      viewer.html + viewer.js                    │
│    auth, device tabs        ├─ viewer-vnc.js      (noVNC / RFB)      │
│                             ├─ viewer-stream.js   (canvas + gestures)│
│                             ├─ viewer-ios.js      (WDA commands)     │
│                             └─ viewer-android.js  (adb commands)     │
└───────────────┬──────────────────────────────────┬───────────────────┘
                │ fetch (JSON, CSRF header)        │ WebSocket
                ▼                                  ▼
┌──────────────────────────────────────────────────────────────────────┐
│ sethu (Rust, axum 0.8, tokio)                                        │
│                                                                      │
│  security::guard ── Origin/Host check, CSRF, auth rate limits        │
│  api            ── session, auth, hosts, health                      │
│  ws/vnc         ── bidirectional byte pump (RFB is opaque)            │
│  ws/ios         ── WDA frame producer + command dispatcher           │
│  ws/android     ── adb frame producer + command dispatcher           │
│  web/assets     ── embedded frontend (include_bytes!)                │
│  store          ── RwLock<Db> + atomic JSON writes                   │
└───────────────┬──────────────────┬──────────────────┬────────────────┘
                │ TCP :5900        │ HTTP :8100       │ adb (USB/TCP)
                ▼                  ▼                  ▼
     macOS Screen Sharing    WebDriverAgent       Android device
     Windows VNC server      iPhone / iPad        phone / tablet
```

## Request lifecycle

1. `TcpListener` accepts a connection; `axum::serve` runs it with connect info so
   the rate limiter can see the client IP.
2. `TraceLayer` logs the request, `CompressionLayer` handles text responses.
3. `security::guard` runs for `/api` and `/ws`:
   - If an `Origin` header is present it must match the request `Host` (or an
     explicitly configured origin / `SETHU_PUBLIC_URL`).
   - Unsafe API methods require `x-sethu-csrf` to equal the session's CSRF token,
     except login/register which are rate limited instead.
4. The handler runs. Authenticated handlers extract `AuthUser`, which resolves
   the session cookie → hashed session record → user.
5. `security::hardening_headers` adds CSP and friends to every response,
   including errors and static files.
6. Static requests fall through to `web::serve`, which checks
   `SETHU_WEB_DIR` (development) then compiled-in assets.

## Storage

`SETHU_DATA_DIR/sethu.json`, mode `0600`:

```jsonc
{
  "users": [
    {"id": "uuid", "email": "owner@example.com",
     "password_hash": "$argon2id$...", "created_at": 1730000000}
  ],
  "hosts": [
    {"id": "uuid", "owner_id": "uuid", "name": "Studio Mac",
     "kind": "macos",              // "macos" | "windows" | "ios" | "android"
     "target": "studio-mac.local:5900",
     "notes": "living room", "created_at": 1730000001}
  ],
  "sessions": [
    {"token_hash": "sha256(token)", "csrf": "random-hex",
     "user_id": "uuid", "created_at": 1730000002, "expires_at": 1730604802}
  ],
  "files": [
    {"id": "uuid", "owner_id": "uuid", "name": "report.pdf",
     "disk_name": "uuid", "size": 12345, "mime": "application/pdf",
     "created_at": 1730000003}
  ]
}
```

File contents live under `SETHU_DATA_DIR/files/<owner_id>/<disk_name>`. The
metadata is in the JSON document; the bytes are streamed separately.

Mutations clone the in-memory document, apply the change, serialise to
`sethu.json.tmp`, `fsync`-free rename over the original, then swap the in-memory
copy. A failed write therefore never advances memory past disk. The file is
rewritten on every mutation; for the expected dataset (tens of devices) this is
far cheaper than operating a database, and backups are a single file copy.

Sessions are pruned lazily on startup and whenever a new session is created.

## VNC bridge (`/ws/vnc/{id}`)

The WebSocket carries raw RFB. The server:

1. Verifies the caller owns the host and that it is a VNC-backed kind
   (`macos` or `windows`).
2. Opens `TcpStream` **before** upgrading so unreachable devices produce an
   HTTP 502 instead of a dead socket.
3. Splits both sides and pumps bytes in both directions with a 64 KiB buffer.
   Binary frames map 1:1 to TCP reads/writes. Text frames are reserved (only
   `"ping"` is recognised) and otherwise ignored.
4. Closes both sides when either direction ends.

noVNC handles both authentication styles macOS exposes: Apple Remote Desktop
account auth (RFB security type 30) and the legacy VNC password (type 2). The
viewer asks for a username only when the Mac requires one.

Consequences: the server never decodes RFB, never sees the VNC password in
plaintext, and automatically supports every encoding noVNC implements. The
trade-off is that bandwidth and latency are bounded by the VNC server and the
WebSocket, not by Sethu.

## iPhone bridge (`/ws/ios/{id}`)

1. Verify ownership and `kind == ios`.
2. Fetch the first screenshot and `/wda/screen` (for the point scale) before
   upgrading; failures return 502 with a useful message.
3. Send a `ready` text frame, then a continuous stream of binary PNG frames.
4. One producer task polls WDA at the requested frame rate. A bounded channel
   (capacity 2) means a slow browser drops frames instead of growing memory.
   Repeated failures back off up to 2 s.
5. Input commands are JSON text frames.

### Server → client

```jsonc
{"type":"ready","width":1170,"height":2532,"scale":3,"fps":10}
{"type":"size","width":2532,"height":1170}          // rotation / size change
{"type":"error","message":"WebDriverAgent: ..."}
```

plus binary PNG frames.

### Client → server

```jsonc
{"type":"tap","x":195,"y":420}                       // logical points
{"type":"swipe","x1":100,"y1":600,"x2":100,"y2":200,"duration":0.3}
{"type":"keys","text":"hello"}
{"type":"button","name":"home"}                      // home | lock | unlock | volumeUp
{"type":"set_fps","value":8}
{"type":"ping"}
```

Coordinates are in **points**, not screenshot pixels. The client divides canvas
pixel coordinates by `scale` from `ready` before sending. The server validates
that coordinates are finite, non-negative and inside the last known frame.

## WDA endpoints used

| Action | Endpoint |
| --- | --- |
| Screenshot | `GET /screenshot` |
| Screen size (points) | `GET /wda/screen` |
| Status | `GET /status` |
| Tap | `POST /wda/tap` |
| Swipe | `POST /wda/dragfromtoforduration` |
| Text | `POST /wda/keys` |
| Home | `POST /wda/homescreen` |
| Lock / unlock | `POST /wda/device/lock`, `/wda/device/unlock` |
| Other buttons | `POST /wda/pressButton` |

These are session-less routes available in stock WebDriverAgent; no Appium
server and no WDA session creation are needed.

## Android bridge (`/ws/android/{id}`)

Android uses the platform `adb` executable through `adb.rs`:

1. Verify ownership and `kind == android`.
2. Run `adb -s <serial> exec-out screencap -p` before upgrading. Failures return
   HTTP 502 with the adb error (device offline, unauthorized, missing binary).
3. Send `ready` with the PNG dimensions and `scale: 1` (adb input uses pixels),
   then stream PNG frames from a bounded producer task.
4. Translate commands into `adb shell input` calls:
   - `tap` → `input tap x y`
   - `swipe` → `input swipe x1 y1 x2 y2 <ms>`
   - `keys` → `input text <encoded>`
   - `button` → `input keyevent <code>` (home/back/recents/power/wake/volume)
5. `SETHU_ANDROID_FPS` controls the polling rate (default 5). Failures back off
   so a disconnected phone is not hammered.

`adb shell input text` passes through the device shell, so Sethu only accepts a
safe character set (letters, digits, spaces, common punctuation) and encodes
spaces as `%s`. Anything else is rejected rather than escaped.

### Server → client

Same control frames as the iPhone bridge: `ready`, `size`, `error`, plus binary
PNG frames.

### Client → server

```jsonc
{"type":"tap","x":540,"y":1200}                     // pixels
{"type":"swipe","x1":100,"y1":1600,"x2":100,"y2":400,"duration":0.3}
{"type":"keys","text":"hello world"}
{"type":"button","name":"home"}                     // home | back | recents | power | wake
{"type":"set_fps","value":5}
{"type":"ping"}
```

## Shared files

`files.rs` is a separate router with its own body limit:

- `POST /api/files` accepts one file per request (the UI sends files
  sequentially). The filename is reduced to a sanitised basename (180 chars max,
  control characters removed, any extension allowed) and the bytes stream to
  `data/files/<owner>/<uuid>`.
- The handler enforces `SETHU_MAX_FILE_BYTES` while streaming and deletes the
  partial file on error or over-limit.
- `GET /api/files` lists metadata; `GET /api/files/{id}/download` streams with
  `Content-Disposition: attachment` and `nosniff`; `DELETE /api/files/{id}`
  removes metadata and contents.
- `POST /api/files/{id}/send` pushes a file to a configured Android host with
  `adb push` into `/sdcard/Download/`. Browsers that cannot be pushed to use the
  copyable download link instead.
- JSON endpoints keep a 64 KiB body limit; the file router disables it and relies
  on the streaming cap. This is verified by an integration test that uploads
  100 KB.

This is a shared server workspace, not peer-to-peer transfer. Upload from a
phone, download on the MacBook (or vice versa). It is not end-to-end encrypted,
and the server operator can read stored files. The frontend exposes it as the
**Files** tab next to **Devices**.

## Sign-in modes

- `SETHU_AUTH=required` — email + password, Argon2id, registration.
- `SETHU_AUTH=username` — the login handler accepts only `username`; unknown
  names create a user row automatically. Sessions and CSRF stay enabled, but
  identity is not verified. The frontend hides the tabs, password field and
  registration controls and posts `{"username": "..."}`.
- `SETHU_AUTH=open` — every request resolves to the first account (creating a
  local owner if the server has none), hides the sign-in screen and skips CSRF
  and the login/registration endpoints. Origin/Host checks still apply. Intended
  for a single-user LAN/VPN deployment; never expose it directly to the internet.

## QR pairing

`pairing.rs` implements a short-lived session handoff:

1. An authenticated browser calls `POST /api/pair` (optionally with `host_id`).
2. The server stores a 24-byte token in memory with the owner, optional host and
   expiry (`SETHU_PAIR_TTL_SECONDS`, default 120 s), and renders an SVG QR code
   as a `data:` URI.
3. The QR encodes `{public_url}/pair/{token}`. Without `SETHU_PUBLIC_URL`, the
   server uses `X-Forwarded-*` (when trusted) or `Host`, and replaces
   `localhost`/`127.0.0.1` with the host's LAN IP so a phone can reach it.
4. Opening the link shows a confirmation page (GET). Link previews can fetch
   this without consuming the token. The page's `pair.js` auto-submits, the
   POST consumes the token, creates a session, sets the cookie and redirects to
   `/` or `/viewer.html?host=…`. Replaying the token returns 404.

The phone can also scan inside the app: jsQR decodes the camera stream on HTTPS,
or a photo/pasted URL on plain HTTP. Camera access is blocked by browsers on
insecure LAN origins, which is why the photo/paste fallbacks exist.

The token is a credential: it is single-use, expires quickly, and is only
created by an authenticated (or open-mode) browser. It is held in memory, so a
restart invalidates outstanding codes.

## Connection resilience

Both viewer families are self-healing once a session exists:

- **VNC** (`viewer-vnc.js`): on disconnect or a failed connect it retries with
  1/2/4/8/15s backoff, drops the old RFB object, recreates it, and reapplies
  scale/view-only/quality preferences. Credentials entered in the dialog are
  cached in page memory and replayed on reconnect; `securityfailure` clears them.
  The VNC bridge sends WebSocket pings every 25s.
- **Screenshot streams** (`viewer-stream.js`): a new WebSocket is created on
  close, with the same backoff. A 15s application ping keeps intermediaries
  awake, and a watchdog reconnects if no frame arrives for 8s. `online`,
  `offline` and `visibilitychange` drive immediate recovery.
- Both use the existing HttpOnly session cookie, so reconnects never need the QR
  code again. Wake locks are requested on secure origins.

## Frontend

Plain ES modules and one stylesheet. No bundler, no framework, no build step.

- `app.js` handles boot, login/register, device CRUD, platform filter tabs,
  the shared **Files** workspace (drag/drop, sequential uploads with progress,
  downloads, delete) and toasts. It never uses `innerHTML` with user data; DOM
  nodes are constructed and filled with `textContent`.
- `viewer.js` is the shell: loads the host, picks a viewer module, renders its
  action buttons, handles fullscreen/disconnect/reconnect.
- `viewer-vnc.js` dynamically imports noVNC from `/vendor/novnc/core/rfb.js` if
  vendored, otherwise from jsDelivr. The server's CSP allows both.
- `viewer-stream.js` is the shared screenshot engine for iPhone and Android:
  frame drain loop (no out-of-order paints), gesture mapping, fps stats and
  screenshot download.
- `viewer-ios.js` / `viewer-android.js` supply the device-specific toolbars and
  WebSocket paths.

The embedded assets are compiled with `include_bytes!`, which keeps deployment
to one file. `SETHU_WEB_DIR` overrides them for development.

## Testing

- Unit tests for password/email rules, target normalisation, session hashing,
  rate limiting, PNG parsing, WDA URL joining, adb text encoding and iPhone /
  Android command decoding.
- Integration tests drive the real axum router with `tower::ServiceExt`:
  registration, duplicate accounts, login timing paths, CSRF rejection, origin
  rejection, host CRUD and logout invalidation.
- WebSocket tests use a fake RFB server and a fake `adb` executable to verify
  byte relay, frame streaming, coordinate conversion and input injection.
- File tests cover multipart uploads above the JSON body limit, download bytes
  and headers, deletion, oversize rejection with cleanup, sanitised filenames
  and open-access mode.- No test needs a Mac, a phone or network access.

## Design choices and trade-offs

| Choice | Why | Cost |
| --- | --- | --- |
| JSON store instead of SQLite | zero native deps, easy backups, auditable | entire file rewritten per change |
| Raw RFB passthrough | full encoding support, no secrets server-side | no server-side recording/transcoding |
| Screenshot polling for iOS/Android | works with stock WDA and adb, no jailbreak/root | low fps, higher bandwidth |
| Vanilla frontend | no toolchain, easy audits and forks | fewer UI conveniences |
| In-memory rate limiting | simple and dependency-free | resets on restart, per-process only |
| Same-origin checks + CSRF | defence in depth, blocks drive-by WebSocket use | needs correct `SETHU_PUBLIC_URL` behind proxies |
