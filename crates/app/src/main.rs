mod state;

use platform::config::Settings;
use platform::events::run_dispatcher;
use platform::observability::{correlation_id_middleware, init_tracing};
use platform::server::cors_layer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing("info");
    let settings = Settings::load()?;

    let res = state::build_resources(settings).await?;
    let port = res.settings.server.port;
    let cors = cors_layer(&res.settings.cors_allowed_origins);

    let app = domain_account::router(state::account_state(&res))
        .merge(domain_auth::router(state::auth_state(&res)))
        .layer(axum::middleware::from_fn(correlation_id_middleware))
        .layer(cors);

    let (pool, registry, dispatcher_cfg, interval) = state::dispatcher_handle(&res);
    let dispatcher = tokio::spawn(run_dispatcher(pool, registry, dispatcher_cfg, interval));

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
        _ = dispatcher => { tracing::error!("dispatcher exited unexpectedly"); }
        _ = pruner => { tracing::error!("prune task exited unexpectedly"); }
    }
    Ok(())
}
