use bh_rust::{config, dns, handlers};
use std::sync::Arc;
use chrono::Utc;

use std::net::SocketAddr;

use axum::{
    routing::get,
    Router,
};
use config::AppConfig;
use handlers::AppState;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bh_rust=info".parse()?),
        )
        .init();

    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    let config_path = args
        .iter()
        .position(|a| a == "--config" || a == "-c")
        .map(|i| args.get(i + 1).expect("--config requires a path").clone())
        .unwrap_or_else(|| "config.toml".to_string());

    // Load config
    let cfg = config::load_config(&config_path)?;
    info!(
        host = %cfg.server.host,
        port = cfg.server.port,
        resize_short_side = cfg.image.resize_short_side,
        quality = cfg.image.quality,
        proxy = ?cfg.proxy.as_ref().map(|p| &p.address),
        "Starting bh-rust"
    );

    // Build HTTP client (with SOCKS5 proxy if configured)
    let client = build_client(&cfg)?;

    // Build router
    let started_on = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);

    let state = AppState {
        config: cfg.clone(),
        client,
        started_on,
        stats: Arc::new(handlers::Stats::new()),
    };

    let app = Router::new()
        .route("/", get(handlers::handle_compress))
        .route("/health", get(handlers::handle_health))
        .route("/stats", get(handlers::handle_stats))
        .with_state(state);

    // Listen
    let addr: SocketAddr = format!("{}:{}", cfg.server.host, cfg.server.port)
        .parse()
        .expect("Invalid host:port");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

/// Создать reqwest::Client с SOCKS5 прокси и SSRF-защитой.
fn build_client(cfg: &AppConfig) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::ClientBuilder::new()
        .dns_resolver(std::sync::Arc::new(dns::SsrfDnsResolver))
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(15))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .user_agent("BandwidthHero/1.0");

    if let Some(proxy_cfg) = &cfg.proxy {
        let mut proxy_url = format!("socks5://{}", proxy_cfg.address);
        if let (Some(user), Some(pass)) = (&proxy_cfg.username, &proxy_cfg.password) {
            proxy_url = format!("socks5://{}:{}@{}", user, pass, proxy_cfg.address);
        }

        info!(address = %proxy_cfg.address, "Configuring SOCKS5 proxy");

        builder = builder.proxy(
            reqwest::Proxy::all(&proxy_url)
                .map_err(|e| anyhow::anyhow!("Invalid proxy URL: {}", e))?,
        );
    }

    builder.build().map_err(Into::into)
}
