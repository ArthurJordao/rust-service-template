use domain_notification::models::NewSentNotification;
use domain_notification::ports::postgres::PostgresSentNotificationRepository;
use domain_notification::ports::repository::SentNotificationRepository;

fn new_row(event_id: i64) -> NewSentNotification {
    NewSentNotification {
        source_event_id: event_id,
        template: "welcome".into(),
        subject: "Welcome".into(),
        channel: "email".into(),
        recipient: "a@b.c".into(),
        body: "hi".into(),
        created_by_cid: "cid".into(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn record_find_list(pool: sqlx::PgPool) {
    let repo = PostgresSentNotificationRepository::new(pool.clone());
    assert!(repo.find_by_event_id(42).await.unwrap().is_none());

    repo.record(new_row(42)).await.unwrap();
    let found = repo.find_by_event_id(42).await.unwrap().unwrap();
    assert_eq!(found.recipient, "a@b.c");
    assert_eq!(found.source_event_id, 42);
    assert_eq!(found.subject, "Welcome");
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
