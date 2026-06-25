use platform::events::{list_dead_letters, replay_dead_letter};

#[sqlx::test(migrations = "../../migrations")]
async fn list_and_replay_dead_letters(pool: sqlx::PgPool) {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('user.registered', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let delivery_id: i64 = sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, last_error) \
         values ($1, 'sub', 'dead', 5, 'boom') returning id",
    )
    .bind(event_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let dead = list_dead_letters(&pool).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].delivery_id, delivery_id);
    assert_eq!(dead[0].last_error.as_deref(), Some("boom"));

    assert!(replay_dead_letter(&pool, delivery_id).await.unwrap());

    let status: String = sqlx::query_scalar("select status from outbox_delivery where id = $1")
        .bind(delivery_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    assert!(list_dead_letters(&pool).await.unwrap().is_empty());
}
