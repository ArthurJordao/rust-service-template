use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::MfaRepository;

async fn seed_user(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) \
         values ('a@b.c', 'x', 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn factor_lifecycle(pool: sqlx::PgPool) {
    let repo = PostgresUserRepository::new(pool.clone());
    let uid = seed_user(&pool).await;

    assert!(repo.confirmed_factor(uid).await.unwrap().is_none());
    repo.upsert_unconfirmed_factor(uid, "totp", b"enc")
        .await
        .unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_none()); // unconfirmed
    let f = repo.get_factor(uid, "totp").await.unwrap().unwrap();
    assert_eq!(f.secret_encrypted, b"enc");

    repo.confirm_factor(uid, "totp").await.unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_some());

    repo.store_recovery_codes(uid, &["h1".into(), "h2".into()])
        .await
        .unwrap();
    // consume matching a hash we can verify is impossible here (hashes are bcrypt);
    // instead test delete path:
    repo.delete_factors(uid).await.unwrap();
    repo.delete_recovery_codes(uid).await.unwrap();
    assert!(repo.confirmed_factor(uid).await.unwrap().is_none());
}
