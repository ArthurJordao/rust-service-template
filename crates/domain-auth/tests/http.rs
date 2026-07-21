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

fn state(pool: sqlx::PgPool) -> AuthState {
    let repo = Arc::new(PostgresUserRepository::new(pool.clone()));
    AuthState {
        pool: pool.clone(),
        users: repo.clone(),
        refresh_tokens: repo.clone(),
        scopes: repo.clone(),
        publisher: Arc::new(OutboxPublisher::new(
            Routes::new().add("user.registered", "account.on-user-registered"),
        )),
        issuer: Arc::new(JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap()),
        verifier: Arc::new(platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(platform::auth::NoopRevocationChecker),
        admin_emails: Arc::new(vec![]),
        metrics: Metrics::new().unwrap(),
        mfa: repo.clone(),
        mfa_verifier: Arc::new(domain_auth::auth::totp::TotpVerifier::new("test".into())),
        mfa_config: domain_auth::ports::http::MfaConfig {
            policy: platform::config::MfaPolicy::Off,
            cipher: None,
        },
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_then_login(pool: sqlx::PgPool) {
    let app = router(state(pool.clone()));

    // Register.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let register_set_cookie = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| v.to_str().ok().filter(|s| s.starts_with("rt=")))
        .expect("register sets an rt cookie")
        .to_string();
    assert!(register_set_cookie.contains("HttpOnly"));
    assert!(register_set_cookie.contains("Secure"));
    assert!(register_set_cookie.contains("SameSite=Strict"));
    assert!(register_set_cookie.contains("Path=/api/auth"));
    let register_body = res.into_body().collect().await.unwrap().to_bytes();
    let register_json: serde_json::Value = serde_json::from_slice(&register_body).unwrap();
    assert!(
        register_json["refresh_token"].is_null(),
        "refresh token must not appear in the register response body"
    );

    // Duplicate email -> 409.
    let dup = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // Login with correct password -> 200 + tokens + rt cookie.
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| v.to_str().ok().filter(|s| s.starts_with("rt=")))
        .expect("login sets an rt cookie")
        .to_string();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/api/auth"));
    let body = login.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "authenticated");
    assert!(json["tokens"]["access_token"].as_str().unwrap().len() > 10);
    assert!(
        json["tokens"]["refresh_token"].is_null(),
        "refresh token must not appear in the response body"
    );

    // Wrong password -> 401.
    let bad = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"wrong"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_persists_refresh_token(pool: sqlx::PgPool) {
    let app = router(state(pool.clone()));
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"hunter2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let n: i64 = sqlx::query_scalar("select count(*) from refresh_token")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}
