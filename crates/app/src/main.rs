use app::state;
use platform::config::Settings;
use platform::events::{run_consumers, DispatcherConfig, ReaperConfig};
use platform::observability::init_tracing;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a local `.env` if present (dev convenience). No-op in prod/CI where the
    // file is absent and real environment variables are used instead.
    dotenvy::dotenv().ok();

    init_tracing("info");
    tracing::info!(
        build_sha = %std::env::var("APP_BUILD_SHA").unwrap_or_default(),
        "starting app"
    );
    let settings = Settings::load()?;

    let res = state::build_resources(settings).await?;
    let port = res.settings.server.port;

    let web_dist = std::path::Path::new("web/dist");
    let web_dist = web_dist.exists().then(|| web_dist.to_path_buf());

    let mut router_cfg = state::RouterConfig::new(res.settings.cors_allowed_origins.clone());
    router_cfg.request_timeout =
        std::time::Duration::from_secs(res.settings.server.request_timeout_seconds);
    router_cfg.max_body_bytes = res.settings.server.max_body_bytes;
    router_cfg.auth_rate_limit_per_minute = res.settings.server.auth_rate_limit_per_minute;
    router_cfg.auth_rate_limit_burst = res.settings.server.auth_rate_limit_burst;

    let app = state::build_router(
        state::account_state(&res),
        state::auth_state(&res),
        state::dlq_state(&res),
        state::notification_state(&res),
        res.metrics.clone(),
        res.pool.clone(),
        router_cfg,
        web_dist,
    );

    use tokio_util::sync::CancellationToken;

    let shutdown = CancellationToken::new();

    // Translate OS signals into a token cancel.
    {
        let s = shutdown.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            tracing::info!("shutdown signal received");
            s.cancel();
        });
    }

    let (pool, registry) = state::consumers_handle(&res);
    let consumers = tokio::spawn(run_consumers(
        pool,
        registry,
        DispatcherConfig::default(),
        ReaperConfig::default(),
        shutdown.clone(),
    ));

    let prune_pool = res.pool.clone();
    let prune_shutdown = shutdown.clone();
    let pruner = tokio::spawn(async move {
        loop {
            if prune_shutdown.is_cancelled() {
                break;
            }
            if let Err(e) =
                domain_auth::ports::revocation::prune_expired_denylist(&prune_pool).await
            {
                tracing::error!(error = %e, "denylist prune failed");
            }
            tokio::select! {
                _ = prune_shutdown.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {}
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP server listening");

    let server_shutdown = shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { server_shutdown.cancelled().await })
        .await?;

    // Server has drained. Ensure background tasks stop, with a bounded wait.
    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let _ = consumers.await;
        let _ = pruner.await;
    })
    .await;
    tracing::info!("shutdown complete");
    Ok(())
}

/// Resolve on SIGTERM (container stop) or SIGINT (Ctrl-C).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
