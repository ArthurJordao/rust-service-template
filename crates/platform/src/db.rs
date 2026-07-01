use crate::config::DatabaseSettings;
use sqlx::postgres::PgPoolOptions;

pub type Db = sqlx::PgPool;

pub async fn make_pool(settings: &DatabaseSettings) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(settings.max_connections)
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
