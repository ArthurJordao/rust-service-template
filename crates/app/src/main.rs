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
    let settings = Settings::load()?;

    let res = state::build_resources(settings).await?;
    let port = res.settings.server.port;

    let web_dist = std::path::Path::new("web/dist");
    let web_dist = web_dist.exists().then(|| web_dist.to_path_buf());

    let app = state::build_router(
        state::account_state(&res),
        state::auth_state(&res),
        state::dlq_state(&res),
        state::notification_state(&res),
        res.metrics.clone(),
        &res.settings.cors_allowed_origins,
        web_dist,
    );

    let (pool, registry) = state::consumers_handle(&res);
    let consumers = tokio::spawn(run_consumers(
        pool,
        registry,
        DispatcherConfig::default(),
        ReaperConfig::default(),
    ));

    let prune_pool = res.pool.clone();
    let pruner = tokio::spawn(async move {
        loop {
            if let Err(e) =
                domain_auth::ports::revocation::prune_expired_denylist(&prune_pool).await
            {
                tracing::error!(error = %e, "denylist prune failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP server listening");
    let server = axum::serve(listener, app);

    tokio::select! {
        r = server => { r?; }
        _ = consumers => { tracing::error!("consumers exited unexpectedly"); }
        _ = pruner => { tracing::error!("prune task exited unexpectedly"); }
    }
    Ok(())
}
