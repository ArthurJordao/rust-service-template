use domain_account::models::NewAccount;
use domain_account::ports::postgres::{create_account_with_event, PostgresAccountRepository};
use domain_account::ports::AccountRepository;
use platform::events::{OutboxPublisher, Routes};

#[sqlx::test(migrations = "../../migrations")]
async fn create_inserts_account_and_emits_event(pool: sqlx::PgPool) {
    let publisher = OutboxPublisher::new(Routes::new());
    let repo = PostgresAccountRepository::new(pool.clone());

    let acc = create_account_with_event(
        &pool,
        &publisher,
        NewAccount { email: "a@b.c".into(), name: "A".into(), auth_user_id: 42 },
        "cid-1",
    )
    .await
    .unwrap();
    assert_eq!(acc.auth_user_id, 42);
    assert_eq!(acc.created_by_cid, "cid-1");

    // Event row written in the same txn.
    let events: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'account.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);

    // Reads via the port.
    assert!(repo.find_by_auth_user_id(42).await.unwrap().is_some());
    assert_eq!(repo.list().await.unwrap().len(), 1);

    // Idempotent: second create returns existing, no duplicate.
    let again = create_account_with_event(
        &pool,
        &publisher,
        NewAccount { email: "a@b.c".into(), name: "A".into(), auth_user_id: 42 },
        "cid-2",
    )
    .await
    .unwrap();
    assert_eq!(again.id, acc.id);
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
