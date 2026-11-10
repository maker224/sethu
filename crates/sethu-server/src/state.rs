use std::sync::Arc;

use anyhow::Context;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{config::Config, rate_limit::RateLimiter, store::Store};

/// Shared application state. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    pub http: reqwest::Client,
    pub limiter: Arc<RateLimiter>,
    /// Caps concurrent VNC/iPhone streams so one account cannot exhaust the
    /// server or the device.
    pub ws_slots: Arc<Semaphore>,
    /// Workspace used when `SETHU_AUTH=open` removes the sign-in screen.
    pub local_user: Option<Uuid>,
}

impl AppState {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let store = Store::open(&cfg.data_dir)
            .with_context(|| format!("opening data dir {}", cfg.data_dir.display()))?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("sethu/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .context("building HTTP client")?;
        let ws_slots = Arc::new(Semaphore::new(cfg.max_sessions));

        let local_user = if cfg.open_access {
            // Reuse the oldest account so devices added before open mode remain
            // visible; create an owner if the server has never had one.
            let user = match store.first_user() {
                Some(user) => user,
                None => store
                    .create_user("local@sethu.invalid", "open-access-mode".into())
                    .with_context(|| "creating the open-access workspace")?,
            };
            Some(user.id)
        } else {
            None
        };

        Ok(Self {
            cfg: Arc::new(cfg),
            store: Arc::new(store),
            http,
            limiter: Arc::new(RateLimiter::new()),
            ws_slots,
            local_user,
        })
    }
}
