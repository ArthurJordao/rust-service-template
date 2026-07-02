use crate::config::DatabaseSettings;
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use std::time::Duration;

pub type Db = sqlx::PgPool;

pub async fn make_pool(settings: &DatabaseSettings) -> anyhow::Result<Db> {
    let statement_timeout_ms = settings.statement_timeout_ms;
    let lock_timeout_ms = settings.lock_timeout_ms;
    let pool = PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .min_connections(settings.min_connections)
        .acquire_timeout(Duration::from_secs(settings.acquire_timeout_seconds))
        .idle_timeout(Duration::from_secs(settings.idle_timeout_seconds))
        .max_lifetime(Duration::from_secs(settings.max_lifetime_seconds))
        .after_connect(move |conn, _meta| {
            Box::pin(async move {
                // Both SETs in one string: sqlx sends a parameterless query via the
                // simple-query protocol, which allows multiple statements. (Don't switch
                // this to query!/prepared statements — those disallow multi-statement.)
                conn.execute(
                    format!(
                        "set statement_timeout = '{statement_timeout_ms}'; \
                         set lock_timeout = '{lock_timeout_ms}';"
                    )
                    .as_str(),
                )
                .await?;
                Ok(())
            })
        })
        .connect(&settings.url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &Db) -> anyhow::Result<()> {
    // `sqlx::migrate!` resolves relative to this crate's manifest dir
    // (`crates/platform`), so `../../migrations` points at the workspace-root
    // `migrations/` directory that is the single source of truth.
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::DatabaseSettings;

    #[test]
    fn builds_settings_struct() {
        // Compile-only guard: a real connection is exercised in integration tests.
        let _s = DatabaseSettings {
            url: "postgres://localhost/x".into(),
            max_connections: 5,
            auto_migrate: false,
            min_connections: 1,
            acquire_timeout_seconds: 5,
            idle_timeout_seconds: 600,
            max_lifetime_seconds: 1800,
            statement_timeout_ms: 10_000,
            lock_timeout_ms: 5_000,
        };
    }
}
