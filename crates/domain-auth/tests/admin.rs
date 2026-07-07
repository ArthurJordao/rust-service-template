use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::ports::http::{router, AuthState};
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> (AuthState, JwtIssuer) {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
    let s = AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        scopes: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
    };
    (s, issuer)
}

fn bearer(issuer: &JwtIssuer, scopes: &[&str]) -> String {
    let (token, _) = issuer
        .issue_access(
            1,
            "admin@x.y",
            scopes.iter().map(|s| s.to_string()).collect(),
            vec!["pwd".into()],
            chrono::Utc::now(),
        )
        .unwrap();
    format!("Bearer {token}")
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_lists_scope_catalog(pool: sqlx::PgPool) {
    let (s, issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/scopes")
                .header("authorization", bearer(&issuer, &["admin"]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "admin"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_admin_is_forbidden(pool: sqlx::PgPool) {
    let (s, issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/scopes")
                .header("authorization", bearer(&issuer, &["read:accounts:own"]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn missing_token_is_unauthorized(pool: sqlx::PgPool) {
    let (s, _issuer) = state(pool);
    let app = router(s);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_sets_user_scopes(pool: sqlx::PgPool) {
    let uid: i64 = sqlx::query_scalar(
        "insert into auth_user (email, password_hash, created_by_cid) values ('u@x.y','h','cid') returning id",
    )
    .fetch_one(&pool).await.unwrap();
    let (s, issuer) = state(pool.clone());
    let app = router(s);

    let res = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/users/{uid}/scopes"))
                .header("authorization", bearer(&issuer, &["admin"]))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scopes":["read:accounts:own"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let n: i64 = sqlx::query_scalar("select count(*) from user_scope where user_id = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}
