use platform::events::claim_batch;

async fn insert_event(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('e', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn claim_flips_to_processing_and_does_not_reclaim(pool: sqlx::PgPool) {
    // Four distinct events, one delivery each for subscriber "s".
    // unique(event_id, subscriber_name) forbids multiple deliveries per event/subscriber.
    for _ in 0..4 {
        let event_id = insert_event(&pool).await;
        sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 's')")
            .bind(event_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let first = claim_batch(&pool, "s", 2).await.unwrap();
    assert_eq!(first.len(), 2);
    let second = claim_batch(&pool, "s", 2).await.unwrap();
    assert_eq!(second.len(), 2);

    // The two claims are disjoint — a claimed (processing) row is never re-handed-out.
    let f: Vec<i64> = first.iter().map(|r| r.delivery_id).collect();
    let s: Vec<i64> = second.iter().map(|r| r.delivery_id).collect();
    assert!(
        f.iter().all(|id| !s.contains(id)),
        "claims overlapped: {f:?} vs {s:?}"
    );

    // All four are now processing, so a further claim sees nothing.
    assert!(claim_batch(&pool, "s", 10).await.unwrap().is_empty());
    let processing: i64 =
        sqlx::query_scalar("select count(*) from outbox_delivery where status = 'processing'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(processing, 4);
}

#[sqlx::test(migrations = "../../migrations")]
async fn claim_is_scoped_to_the_subscriber(pool: sqlx::PgPool) {
    let event_id = insert_event(&pool).await;
    sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 'a')")
        .bind(event_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into outbox_delivery (event_id, subscriber_name) values ($1, 'b')")
        .bind(event_id)
        .execute(&pool)
        .await
        .unwrap();

    let claimed = claim_batch(&pool, "a", 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].subscriber_name, "a");
}
