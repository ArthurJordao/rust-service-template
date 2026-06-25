use domain_auth::models::NewUser;
use domain_auth::ports::postgres::{register_user_with_event, PostgresUserRepository};
use domain_auth::ports::UserRepository;
use platform::events::{OutboxPublisher, Routes};

#[sqlx::test(migrations = "../../migrations")]
async fn register_inserts_user_scopes_and_emits_event(pool: sqlx::PgPool) {
    let publisher = OutboxPublisher::new(Routes::new());
    let repo = PostgresUserRepository::new(pool.clone());

    let user = register_user_with_event(
        &pool,
        &publisher,
        NewUser {
            email: "a@b.c".into(),
            password_hash: "hash".into(),
        },
        &["read:accounts:own"],
        "cid-1",
    )
    .await
    .unwrap();
    assert_eq!(user.email, "a@b.c");
    assert_eq!(user.created_by_cid, "cid-1");

    // user.registered emitted in the same txn.
    let events: i64 = sqlx::query_scalar(
        "select count(*) from outbox_event where event_type = 'user.registered'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);

    // Reads + scopes via the port.
    assert!(repo.find_by_email("a@b.c").await.unwrap().is_some());
    assert_eq!(
        repo.scope_names(user.id).await.unwrap(),
        vec!["read:accounts:own".to_string()]
    );
    assert_eq!(repo.list().await.unwrap().len(), 1);
}
