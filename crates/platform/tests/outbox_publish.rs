use platform::events::{EventPublisher, NewEvent, OutboxPublisher, Routes};

#[sqlx::test(migrations = "../../migrations")]
async fn publish_fans_out_one_delivery_per_subscriber(pool: sqlx::PgPool) {
    let routes = Routes::new()
        .add("user.registered", "sub-a")
        .add("user.registered", "sub-b");
    let publisher = OutboxPublisher::new(routes);

    let mut tx = pool.begin().await.unwrap();
    let event_id = publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: "42".into(),
                payload: serde_json::json!({ "uid": 42, "email": "a@b.c" }),
                correlation_id: "cid-123".into(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let deliveries: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where event_id = $1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deliveries, 2, "one delivery row per subscriber");
}
