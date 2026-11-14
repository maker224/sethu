//! Configuration comes from environment variables so the same binary works in
//! Docker, systemd and local development.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server listens on.
    pub bind: SocketAddr,
    /// Directory holding `sethu.json` (users, hosts, sessions).
    pub data_dir: PathBuf,
    /// Optional directory with frontend files, useful for development. When
    /// unset the compiled-in assets are served.
    pub web_dir: Option<PathBuf>,
    /// Directory containing optional third-party assets such as a vendored copy
    /// of noVNC (`web/vendor/novnc`). Served read-only.
    pub vendor_dir: PathBuf,
    /// Canonical public URL, e.g. `https://remote.example.com`. Used in logs and
    /// as an extra accepted WebSocket/API origin.
    pub public_url: Option<String>,
    /// Mark the session cookie `Secure`. Turn on whenever TLS terminates in
    /// front of Sethu (strongly recommended).
    pub cookie_secure: bool,
    /// Allow self-service account creation.
    pub allow_registration: bool,
    /// Lifetime of a login session.
    pub session_ttl: Duration,
    /// Default screenshots per second for iPhone mirroring (1-15).
    pub ios_fps: u8,
    /// Default screenshots per second for Android mirroring (1-15).
    pub android_fps: u8,
    /// Path to the `adb` executable used for Android devices.
    pub adb_path: String,
    /// Timeout for a single adb command.
    pub adb_timeout: Duration,
    /// Extra origins accepted for browser requests (comma separated).
    pub allowed_origins: Vec<String>,
    /// Trust `X-Forwarded-For` when rate limiting. Only enable behind a proxy.
    pub trust_proxy: bool,
    /// Timeout for a single WebDriverAgent HTTP call.
    pub wda_timeout: Duration,
    /// Timeout for opening a TCP connection to a VNC server.
    pub vnc_connect_timeout: Duration,
    /// Maximum number of concurrent device streams.
    pub max_sessions: usize,
    /// Skip the sign-in screen and share one workspace between all visitors.
    /// Only appropriate on a trusted LAN / behind a VPN.
    pub open_access: bool,
    /// Sign in with a username only; no password and no registration step.
    pub username_only: bool,
    /// Maximum size of a single uploaded file, in bytes.
    pub max_file_bytes: u64,
    /// Lifetime of a QR pairing code.
    pub pair_ttl: Duration,
    /// How long a browser counts as online after its last heartbeat.
    pub presence_ttl: Duration,
    /// How long a device-to-device transfer waits for the receiver to accept.
    pub transfer_ttl: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let default_bind = format!("0.0.0.0:{}", env("PORT", "8080"));
        let bind = env("SETHU_BIND", &default_bind)
            .parse()
            .context("SETHU_BIND must look like 0.0.0.0:8080")?;

        let data_dir = env("SETHU_DATA_DIR", "./data").into();
        let web_dir = std::env::var("SETHU_WEB_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let vendor_dir = std::env::var("SETHU_VENDOR_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./web/vendor"));
        let public_url = std::env::var("SETHU_PUBLIC_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                // Render (and similar PaaS) expose the canonical URL.
                std::env::var("RENDER_EXTERNAL_URL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .map(|v| v.trim().trim_end_matches('/').to_string());

        if let Some(url) = &public_url {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                bail!("SETHU_PUBLIC_URL must start with http:// or https://");
            }
        }

        let cookie_secure = match public_url.as_deref() {
            Some(url) if url.starts_with("https://") => env_bool("SETHU_COOKIE_SECURE", true),
            _ => env_bool("SETHU_COOKIE_SECURE", false),
        };

        let allowed_origins = std::env::var("SETHU_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let auth_mode = env("SETHU_AUTH", "required").to_ascii_lowercase();
        let (open_access, username_only) = match auth_mode.as_str() {
            "required" | "on" => (false, false),
            "username" | "simple" => (false, true),
            "open" | "none" | "off" | "disabled" => (true, false),
            other => bail!("SETHU_AUTH must be `required`, `username` or `open`, got `{other}`"),
        };

        Ok(Self {
            bind,
            data_dir,
            web_dir,
            vendor_dir,
            public_url,
            cookie_secure,
            allow_registration: env_bool("SETHU_ALLOW_REGISTRATION", true),
            session_ttl: Duration::from_secs(env_u64("SETHU_SESSION_TTL_HOURS", 168) * 3_600),
            ios_fps: env_u64("SETHU_IOS_FPS", 10).clamp(1, 15) as u8,
            android_fps: env_u64("SETHU_ANDROID_FPS", 5).clamp(1, 15) as u8,
            adb_path: env("SETHU_ADB_PATH", "adb"),
            adb_timeout: Duration::from_secs(env_u64("SETHU_ADB_TIMEOUT_SECONDS", 10).max(1)),
            allowed_origins,
            trust_proxy: env_bool(
                "SETHU_TRUST_PROXY",
                std::env::var("RENDER_EXTERNAL_URL").is_ok(),
            ),
            wda_timeout: Duration::from_secs(env_u64("SETHU_WDA_TIMEOUT_SECONDS", 6).max(1)),
            vnc_connect_timeout: Duration::from_secs(
                env_u64("SETHU_VNC_CONNECT_TIMEOUT_SECONDS", 10).max(1),
            ),
            max_sessions: env_u64("SETHU_MAX_SESSIONS", 32).clamp(1, 512) as usize,
            open_access,
            username_only,
            max_file_bytes: env_u64("SETHU_MAX_FILE_BYTES", 2 * 1024 * 1024 * 1024).max(1),
            pair_ttl: Duration::from_secs(env_u64("SETHU_PAIR_TTL_SECONDS", 120).clamp(30, 600)),
            presence_ttl: Duration::from_secs(
                env_u64("SETHU_PRESENCE_TTL_SECONDS", 45).clamp(15, 300),
            ),
            transfer_ttl: Duration::from_secs(
                env_u64("SETHU_TRANSFER_TTL_SECONDS", 600).clamp(60, 3_600),
            ),
        })
    }

    /// Insecure, disposable config for tests.
    pub fn for_tests(data_dir: PathBuf) -> Self {
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir,
            web_dir: None,
            vendor_dir: PathBuf::from("./web/vendor"),
            public_url: None,
            cookie_secure: false,
            allow_registration: true,
            session_ttl: Duration::from_secs(3_600),
            ios_fps: 10,
            android_fps: 5,
            adb_path: "adb".into(),
            adb_timeout: Duration::from_secs(2),
            allowed_origins: vec![],
            trust_proxy: false,
            wda_timeout: Duration::from_secs(2),
            vnc_connect_timeout: Duration::from_secs(2),
            max_sessions: 32,
            open_access: false,
            username_only: false,
            max_file_bytes: 10 * 1024 * 1024,
            pair_ttl: Duration::from_secs(120),
            presence_ttl: Duration::from_secs(45),
            transfer_ttl: Duration::from_secs(600),
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}
