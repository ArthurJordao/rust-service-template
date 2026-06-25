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
    }
}

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
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (
        json["access_token"].as_str().unwrap().to_string(),
        json["refresh_token"].as_str().unwrap().to_string(),
    )
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
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"refresh_token":"{rt}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["refresh_token"].as_str().unwrap(),
        rt,
        "refresh token must NOT rotate"
    );
    assert!(
        json["access_token"].as_str().unwrap().len() > 10,
        "a fresh access token is issued"
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
                .body(Body::from(format!(
                    r#"{{"refresh_token":"{rt}","access_token":"{at}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"refresh_token":"{rt}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh.status(), StatusCode::UNAUTHORIZED);
}
