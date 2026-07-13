use chrono::Utc;
use domain_auth::ports::postgres::PostgresUserRepository;
use domain_auth::ports::revocation::PostgresRevocationChecker;
use domain_auth::ports::RefreshTokenRepository;
use platform::auth::{AccessClaims, RevocationChecker};

fn claims(sub: &str, jti: &str, iat: usize) -> AccessClaims {
    AccessClaims {
        sub: sub.into(),
        scopes: vec![],
        exp: 9_999_999_999,
        iat,
        jti: jti.into(),
        email: None,
        token_type: "user".into(),
        amr: vec![],
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn denylisted_jti_is_revoked(pool: sqlx::PgPool) {
    sqlx::query("insert into revoked_access_token (jti, expires_at) values ('bad', now() + interval '1 hour')")
        .execute(&pool).await.unwrap();
    let checker = PostgresRevocationChecker::new(pool.clone());
    assert!(checker
        .is_revoked(&claims("user-1", "bad", 0))
        .await
        .unwrap());
    assert!(!checker
        .is_revoked(&claims("user-1", "good", 9_999_999_999))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn token_issued_before_tokens_valid_from_is_revoked(pool: sqlx::PgPool) {
    // Seed a user; bump tokens_valid_from to "now".
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, tokens_valid_from, created_by_cid) \
         values ('a@b.c', 'h', now(), 'cid') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let checker = PostgresRevocationChecker::new(pool.clone());
    // iat = 0 (epoch) is before tokens_valid_from -> revoked.
    assert!(checker
        .is_revoked(&claims(&format!("user-{uid}"), "j", 0))
        .await
        .unwrap());
    // iat far in the future -> not revoked.
    assert!(!checker
        .is_revoked(&claims(&format!("user-{uid}"), "j", 9_999_999_999))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_token_store_find_revoke(pool: sqlx::PgPool) {
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ('a@b.c','h','cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let repo = PostgresUserRepository::new(pool.clone());
    repo.store("jti-1", uid, Utc::now() + chrono::Duration::days(7))
        .await
        .unwrap();
    let found = repo.find_by_jti("jti-1").await.unwrap().unwrap();
    assert!(!found.revoked);
    repo.revoke("jti-1").await.unwrap();
    assert!(repo.find_by_jti("jti-1").await.unwrap().unwrap().revoked);
}
