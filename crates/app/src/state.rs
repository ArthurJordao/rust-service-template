use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::routing::get;
use axum::Router;
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::AccountState;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::revocation::PostgresRevocationChecker;
use domain_auth::AuthState;
use domain_notification::ports::events::NotificationSubscriber;
use domain_notification::ports::notifier::LogNotifier;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::templates::Templates;
use domain_notification::NotificationState;
use platform::auth::{JwtVerifier, RevocationChecker};
use platform::config::Settings;
use platform::db::{self, Db};
use platform::events::{
    dlq_http::{dlq_router, DlqState},
    DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;
use platform::observability::correlation_id_middleware;
use platform::server::{cors_layer, status_handler};
use tower_http::services::{ServeDir, ServeFile};
use utoipa_swagger_ui::SwaggerUi;

/// All shared resources, constructed once at startup.
pub struct Resources {
    pub settings: Settings,
    pub pool: Db,
    pub registry: Arc<SubscriberRegistry>,
    pub publisher: Arc<dyn EventPublisher>,
    pub jwt: Arc<JwtVerifier>,
    pub issuer: Arc<JwtIssuer>,
    pub admin_emails: Arc<Vec<String>>,
    pub metrics: Metrics,
    pub revocation: Arc<dyn RevocationChecker>,
}

/// Static routing table: every (event_type, subscriber_name) pair the system
/// knows about. Declared here so the publisher never depends on subscriber
/// instances — this is what keeps construction linear and cycle-free.
fn routes() -> Routes {
    Routes::new()
        .add("user.registered", "account.on-user-registered")
        .add("account.created", "notification.on-account-created")
}

pub async fn build_resources(settings: Settings) -> anyhow::Result<Resources> {
    let pool = db::make_pool(&settings.database)
        .await
        .context("create db pool")?;

    if settings.database.auto_migrate {
        tracing::info!("running migrations (auto_migrate=true)");
        db::run_migrations(&pool).await.context("run migrations")?;
    }

    let jwt = Arc::new(
        JwtVerifier::from_rsa_pem(&settings.auth.public_key_pem()?)
            .context("parse JWT public key")?,
    );
    let issuer = Arc::new(
        JwtIssuer::from_rsa_pem(
            &settings.auth.private_key_pem()?,
            settings.auth.access_token_ttl_seconds,
            settings.auth.refresh_token_ttl_days,
        )
        .context("parse JWT private key")?,
    );
    let admin_emails = Arc::new(settings.auth.admin_email_list());
    let metrics = Metrics::new().context("init metrics")?;
    let revocation: Arc<dyn RevocationChecker> =
        Arc::new(PostgresRevocationChecker::new(pool.clone()));

    // Linear construction (no cycle):
    // 1) publisher depends only on Routes (plain data),
    let publisher: Arc<dyn EventPublisher> = Arc::new(OutboxPublisher::new(routes()));
    // 2) subscribers depend on the publisher,
    let account_repo = Arc::new(PostgresAccountRepository::new(pool.clone()));
    let mut registry = SubscriberRegistry::new();
    registry.register(Arc::new(AccountSubscriber::new(
        pool.clone(),
        account_repo.clone(),
        publisher.clone(),
    )));
    let templates = std::sync::Arc::new(Templates::new().context("load notification templates")?);
    let notif_repo = Arc::new(PostgresSentNotificationRepository::new(pool.clone()));
    registry.register(Arc::new(NotificationSubscriber::new(
        notif_repo,
        Arc::new(LogNotifier),
        templates,
    )));
    // 3) the registry (subscriber instances) is consumed only by the dispatcher.
    let registry = Arc::new(registry);

    Ok(Resources {
        settings,
        pool,
        registry,
        publisher,
        jwt,
        issuer,
        admin_emails,
        metrics,
        revocation,
    })
}

pub fn auth_state(res: &Resources) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(res.pool.clone()));
    AuthState {
        pool: res.pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        scopes: repo.clone(),
        publisher: res.publisher.clone(),
        issuer: res.issuer.clone(),
        verifier: res.jwt.clone(),
        revocation: res.revocation.clone(),
        admin_emails: res.admin_emails.clone(),
        metrics: res.metrics.clone(),
    }
}

pub fn account_state(res: &Resources) -> AccountState {
    AccountState {
        pool: res.pool.clone(),
        repo: Arc::new(PostgresAccountRepository::new(res.pool.clone())),
        publisher: res.publisher.clone(),
        jwt: res.jwt.clone(),
        metrics: res.metrics.clone(),
        revocation: res.revocation.clone(),
    }
}

pub fn dispatcher_handle(
    res: &Resources,
) -> (Db, Arc<SubscriberRegistry>, DispatcherConfig, Duration) {
    (
        res.pool.clone(),
        res.registry.clone(),
        DispatcherConfig::default(),
        Duration::from_secs(2),
    )
}

pub fn dlq_state(res: &Resources) -> DlqState {
    DlqState {
        pool: res.pool.clone(),
        jwt: res.jwt.clone(),
        revocation: res.revocation.clone(),
    }
}

pub fn notification_state(res: &Resources) -> NotificationState {
    NotificationState {
        repo: Arc::new(PostgresSentNotificationRepository::new(res.pool.clone())),
        jwt: res.jwt.clone(),
        revocation: res.revocation.clone(),
        metrics: res.metrics.clone(),
    }
}

/// Middleware that promotes a 404 + text/html response to 200.
///
/// `ServeDir::not_found_service(ServeFile::new(index))` serves `index.html` for
/// unknown paths, but tower-http 0.6 preserves the outer 404 status from
/// `ServeDir` even though the inner `ServeFile` handler returns 200.  This
/// middleware fixes up the status so browser navigation to SPA routes works.
async fn spa_status_fixup(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut res = next.run(req).await;
    if res.status() == axum::http::StatusCode::NOT_FOUND {
        let is_html = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.starts_with("text/html"))
            .unwrap_or(false);
        if is_html {
            *res.status_mut() = axum::http::StatusCode::OK;
        }
    }
    res
}

/// Assemble the full application router: API under `/api`, infra at root, and an
/// optional static SPA fallback. Pure (no I/O) so it is unit-testable.
pub fn build_router(
    account: AccountState,
    auth: AuthState,
    dlq: DlqState,
    notification: NotificationState,
    metrics: Metrics,
    cors_origins: &[String],
    web_dist: Option<PathBuf>,
) -> Router {
    let api = domain_account::router(account)
        .merge(domain_auth::router(auth))
        .merge(dlq_router(dlq))
        .merge(domain_notification::router(notification));

    let metrics_for_handler = metrics.clone();
    let mut app = Router::new()
        .route("/status", get(status_handler))
        .route(
            "/metrics",
            get(move || {
                let m = metrics_for_handler.clone();
                async move { m.render() }
            }),
        )
        .nest("/api", api);

    if let Some(dir) = web_dist {
        // Serve the SPA: ServeDir handles exact static-asset requests (JS/CSS/
        // images/favicon) with correct MIME types and 200 statuses.  For any
        // client-side route that has no matching file on disk (e.g. /admin/dlq),
        // ServeDir's not_found_service delivers index.html — but tower-http 0.6
        // preserves ServeDir's outer 404 status even though the inner ServeFile
        // returns 200.  The spa_status_fixup middleware corrects this: when the
        // response is 404 + text/html it can only be the SPA shell, so we
        // promote it to 200.
        let index = dir.join("index.html");
        let spa_router = Router::new()
            .fallback_service(ServeDir::new(&dir).not_found_service(ServeFile::new(&index)));

        app = app.fallback_service(spa_router.layer(axum::middleware::from_fn(spa_status_fixup)));
    }

    app = app
        .merge(SwaggerUi::new("/swagger-ui").url("/api/openapi.json", crate::openapi::api_doc()));

    app.layer(axum::middleware::from_fn(correlation_id_middleware))
        .layer(cors_layer(cors_origins))
}
