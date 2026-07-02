use platform::config::DatabaseSettings;
use platform::db::make_pool;

fn settings(url: &str) -> DatabaseSettings {
    DatabaseSettings {
        url: url.to_string(),
        max_connections: 5,
        auto_migrate: false,
        min_connections: 1,
        acquire_timeout_seconds: 5,
        idle_timeout_seconds: 600,
        max_lifetime_seconds: 1800,
        statement_timeout_ms: 10_000,
        lock_timeout_ms: 5_000,
    }
}

#[tokio::test]
async fn make_pool_applies_statement_and_lock_timeouts() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");
    let pool = make_pool(&settings(&url)).await.expect("make_pool");

    // Postgres normalizes 10000ms -> '10s', 5000ms -> '5s'.
    let stmt: String = sqlx::query_scalar("select current_setting('statement_timeout')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stmt, "10s");

    let lock: String = sqlx::query_scalar("select current_setting('lock_timeout')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(lock, "5s");
}
