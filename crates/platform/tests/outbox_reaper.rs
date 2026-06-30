use platform::events::reap_stale;
use std::time::Duration;

async fn insert_processing(pool: &sqlx::PgPool, attempts: i32, age_minutes: i64) -> i64 {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('e', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, updated_at) \
         values ($1, 's', 'processing', $2, now() - ($3 || ' minutes')::interval) returning id",
    )
    .bind(event_id)
    .bind(attempts)
    .bind(age_minutes.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn reaper_resets_stale_and_leaves_fresh(pool: sqlx::PgPool) {
    let stale = insert_processing(&pool, 0, 10).await; // older than 5 min
    let fresh = insert_processing(&pool, 0, 0).await; // within timeout

    let n = reap_stale(&pool, Duration::from_secs(300), 5)
        .await
        .unwrap();
    assert_eq!(n, 1);

    let stale_row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery where id = $1")
            .bind(stale)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stale_row, ("pending".to_string(), 1));

    let fresh_status: String =
        sqlx::query_scalar("select status from outbox_delivery where id = $1")
            .bind(fresh)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fresh_status, "processing");
}

#[sqlx::test(migrations = "../../migrations")]
async fn reaper_dead_letters_at_max_attempts(pool: sqlx::PgPool) {
    let id = insert_processing(&pool, 4, 10).await; // attempts 4, max 5 -> bumped to 5 -> dead

    let n = reap_stale(&pool, Duration::from_secs(300), 5)
        .await
        .unwrap();
    assert_eq!(n, 1);

    let row: (String, i32) =
        sqlx::query_as("select status, attempts from outbox_delivery where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("dead".to_string(), 5));
}
