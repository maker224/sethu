use std::net::SocketAddr;

use anyhow::Context;
use sethu_server::{app, config::Config, state::AppState};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const HELP: &str = "\
Sethu — self-hosted remote access for Mac and iPhone

USAGE:
    sethu [OPTIONS]

OPTIONS:
    -h, --help       Print this help
    -V, --version    Print the version

Configuration is entirely environment based. See .env.example and the README
for SETHU_BIND, SETHU_DATA_DIR, SETHU_PUBLIC_URL, SETHU_COOKIE_SECURE and the
other settings.
";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("sethu {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}` (try --help)"),
        }
    }

    init_tracing();
    let config = Config::from_env().context("reading configuration")?;
    let state = AppState::new(config)?;

    if !state.cfg.cookie_secure {
        tracing::warn!(
            "session cookies are not marked Secure; put Sethu behind TLS \
             (Caddy/nginx) and set SETHU_COOKIE_SECURE=true before exposing it"
        );
    }
    if state.cfg.public_url.is_none() {
        tracing::warn!(
            "SETHU_PUBLIC_URL is not set; set it to your public https:// URL \
             so proxy deployments accept browser origins"
        );
    }
    if state.cfg.open_access {
        tracing::warn!(
            "SETHU_AUTH=open: authentication is disabled. Everyone who can reach \
             this port can control devices and read files. Use a trusted LAN or VPN."
        );
    }
    if state.store.user_count() == 0 {
        tracing::info!("no accounts yet: open the web UI and create the first one");
    }

    let addr = state.cfg.bind_addr();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(
        %addr,
        data_dir = %state.cfg.data_dir.display(),
        version = env!("CARGO_PKG_VERSION"),
        "sethu is listening"
    );

    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serving HTTP")?;

    tracing::info!("shutting down");
    Ok(())
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sethu_server=info,tower_http=warn".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("installing ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
