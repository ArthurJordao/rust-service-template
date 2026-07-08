use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain_auth::auth::jwt::JwtIssuer;
use domain_auth::auth::mfa_crypto::MfaCipher;
use domain_auth::auth::totp::TotpVerifier;
use domain_auth::ports::http::{router, AuthState, MfaConfig};
use domain_auth::ports::postgres::PostgresUserRepository;
use http_body_util::BodyExt;
use platform::config::MfaPolicy;
use platform::events::{OutboxPublisher, Routes};
use platform::metrics::Metrics;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PRIV_PEM: &str = include_str!("fixtures/test_priv.pem");
const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state_with(pool: sqlx::PgPool, policy: MfaPolicy) -> AuthState {
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
        mfa_verifier: Arc::new(TotpVerifier::new("test".into())),
        mfa_config: MfaConfig {
            policy,
            cipher: Some(Arc::new(MfaCipher::new([9u8; 32]))),
        },
    }
}

async fn register(app: &axum::Router, email: &str, password: &str) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
}

async fn post_json(app: &axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_required_no_factor_returns_enroll_challenge(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let (status, body) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "mfa_required");
    assert_eq!(body["purpose"], "enroll");
    assert!(body["mfa_token"].as_str().unwrap().len() > 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_optional_no_factor_authenticates(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Optional));
    register(&app, "a@b.c", "pw").await;
    let (status, body) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "authenticated");
    assert!(body["tokens"]["access_token"].as_str().unwrap().len() > 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_off_authenticates(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Off));
    register(&app, "a@b.c", "pw").await;
    let (status, body) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "authenticated");
}
