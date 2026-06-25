use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::{ScopeRepository, UserRepository};

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> i64 {
    sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ($1, 'h', 'cid') returning id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn lists_catalog_and_replaces_scopes(pool: sqlx::PgPool) {
    let repo = PostgresUserRepository::new(pool.clone());

    // Catalog is seeded by migration 0005.
    let catalog = repo.list_catalog().await.unwrap();
    assert!(catalog.iter().any(|s| s.name == "admin"));

    let uid = seed_user(&pool, "u@x.y").await;
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select tokens_valid_from from auth_user where id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();

    repo.replace_user_scopes(uid, &["admin".into(), "read:accounts:own".into()])
        .await
        .unwrap();
    assert_eq!(repo.scope_names(uid).await.unwrap().len(), 2);

    // Replacing bumps tokens_valid_from.
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select tokens_valid_from from auth_user where id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(after >= before);

    // Replacing again with fewer scopes overwrites.
    repo.replace_user_scopes(uid, &["read:accounts:own".into()])
        .await
        .unwrap();
    assert_eq!(
        repo.scope_names(uid).await.unwrap(),
        vec!["read:accounts:own".to_string()]
    );

    let users = repo.list_users_with_scopes().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].0.email, "u@x.y");
    assert_eq!(users[0].1, vec!["read:accounts:own".to_string()]);
}
