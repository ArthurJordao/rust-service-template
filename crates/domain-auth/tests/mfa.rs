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

async fn post_bearer(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
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

fn current_totp_code(secret: &str) -> String {
    TotpVerifier::new("test".into())
        .current_code(secret, chrono::Utc::now())
        .unwrap()
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

#[sqlx::test(migrations = "../../migrations")]
async fn forced_enroll_flow_issues_tokens_with_amr(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let mfa_token = login["mfa_token"].as_str().unwrap().to_string();

    // setup with the enroll token
    let (s1, setup) = post_bearer(&app, "/auth/mfa/setup", &mfa_token, "{}").await;
    assert_eq!(s1, 200);
    let secret = setup["secret"].as_str().unwrap().to_string();

    // compute a valid code for `secret` (test TotpVerifier) and confirm
    let code = current_totp_code(&secret);
    let (s2, confirm) = post_bearer(
        &app,
        "/auth/mfa/confirm",
        &mfa_token,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(s2, 200);
    assert_eq!(confirm["recovery_codes"].as_array().unwrap().len(), 10);
    let access_token = confirm["tokens"]["access_token"].as_str().unwrap();
    assert!(access_token.len() > 10);

    // amr present in the issued access token
    let verifier = platform::auth::JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap();
    let claims = verifier.verify(access_token).unwrap();
    assert!(claims.amr.contains(&"totp".to_string()));
    assert!(claims.amr.contains(&"pwd".to_string()));
}
