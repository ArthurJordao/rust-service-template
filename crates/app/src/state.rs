use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use domain_account::ports::events::AccountSubscriber;
use domain_account::ports::postgres::PostgresAccountRepository;
use domain_account::AccountState;
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::AuthState;
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::config::Settings;
use platform::db::{self, Db};
use platform::events::{
    DispatcherConfig, EventPublisher, OutboxPublisher, Routes, SubscriberRegistry,
};
use platform::metrics::Metrics;

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
}

/// Static routing table: every (event_type, subscriber_name) pair the system
/// knows about. Declared here so the publisher never depends on subscriber
/// instances — this is what keeps construction linear and cycle-free.
fn routes() -> Routes {
    Routes::new().add("user.registered", "account.on-user-registered")
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
        JwtVerifier::from_rsa_pem(&settings.auth.jwt_public_key_pem)
            .context("parse JWT public key")?,
    );
    let issuer = Arc::new(
        JwtIssuer::from_rsa_pem(
            &settings.auth.jwt_private_key_pem,
            settings.auth.access_token_ttl_seconds,
            settings.auth.refresh_token_ttl_days,
        )
        .context("parse JWT private key")?,
    );
    let admin_emails = Arc::new(settings.auth.admin_email_list());
    let metrics = Metrics::new().context("init metrics")?;

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
    })
}

pub fn auth_state(res: &Resources) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(res.pool.clone()));
    AuthState {
        pool: res.pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        publisher: res.publisher.clone(),
        issuer: res.issuer.clone(),
        verifier: res.jwt.clone(),
        // TODO(2b Task 6): swap to PostgresRevocationChecker
        revocation: Arc::new(NoopRevocationChecker),
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
        // TODO(2b Task 6): swap to PostgresRevocationChecker
        revocation: Arc::new(NoopRevocationChecker),
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
