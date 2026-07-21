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
        publisher: Arc::new(OutboxPublisher::new(Routes::new())),
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

/// Extract the `rt=<value>` portion out of a `Set-Cookie` header value (ignoring
/// the trailing attributes).
fn rt_value_from_set_cookie(set_cookie: &str) -> String {
    set_cookie
        .strip_prefix("rt=")
        .expect("cookie is the rt cookie")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string()
}

fn find_rt_set_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all("set-cookie")
        .iter()
        .find_map(|v| v.to_str().ok().filter(|s| s.starts_with("rt=")))
        .map(str::to_string)
}

/// Register a user and return (access_token, rt cookie value) — the refresh
/// token now arrives only via the `Set-Cookie: rt=...` header, not the body.
async fn register_and_get_tokens(app: &axum::Router) -> (String, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/register")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"a@b.c","password":"pw"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let set_cookie = find_rt_set_cookie(res.headers()).expect("register sets an rt cookie");
    let rt = rt_value_from_set_cookie(&set_cookie);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (json["access_token"].as_str().unwrap().to_string(), rt)
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_returns_new_access_token(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let (_at, rt) = register_and_get_tokens(&app).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("cookie", format!("rt={rt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let set_cookie = find_rt_set_cookie(res.headers()).expect("refresh re-sets the rt cookie");
    assert_eq!(
        rt_value_from_set_cookie(&set_cookie),
        rt,
        "refresh token must NOT rotate"
    );
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.contains("Path=/api/auth"));

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["refresh_token"].is_null(),
        "refresh token must not appear in the response body"
    );
    assert!(
        json["access_token"].as_str().unwrap().len() > 10,
        "a fresh access token is issued"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_without_cookie_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "no rt cookie => unauthorized"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn logout_then_refresh_is_unauthorized(pool: sqlx::PgPool) {
    let app = router(state(pool));
    let (at, rt) = register_and_get_tokens(&app).await;

    let logout = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("content-type", "application/json")
                .header("cookie", format!("rt={rt}"))
                .body(Body::from(format!(r#"{{"access_token":"{at}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let clear_cookie =
        find_rt_set_cookie(logout.headers()).expect("logout re-sets the rt cookie to clear it");
    assert!(
        clear_cookie.contains("Max-Age=0"),
        "logout must clear the rt cookie: {clear_cookie}"
    );

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("cookie", format!("rt={rt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn access_token_rejected_at_refresh_endpoint(pool: sqlx::PgPool) {
    // An access token must be rejected by /auth/refresh (wrong token type).
    let app = router(state(pool));
    let (at, _rt) = register_and_get_tokens(&app).await;

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("cookie", format!("rt={at}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "access token must not be accepted at /auth/refresh"
    );
}
