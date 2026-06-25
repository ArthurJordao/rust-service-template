use axum::body::Body;
use axum::http::{Request, StatusCode};
use platform::auth::{JwtVerifier, NoopRevocationChecker};
use platform::events::dlq_http::{dlq_router, DlqState};
use std::sync::Arc;
use tower::ServiceExt;

const TEST_PUB_PEM: &str = include_str!("fixtures/test_pub.pem");

fn state(pool: sqlx::PgPool) -> DlqState {
    DlqState {
        pool,
        jwt: Arc::new(JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap()),
        revocation: Arc::new(NoopRevocationChecker),
    }
}

async fn seed_dead(pool: &sqlx::PgPool) -> i64 {
    let event_id: i64 = sqlx::query_scalar(
        "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
         values ('user.registered', '1', '{}'::jsonb, 'cid') returning id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "insert into outbox_delivery (event_id, subscriber_name, status, attempts, last_error) \
         values ($1, 'sub', 'dead', 5, 'boom') returning id",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_dlq_without_token_is_unauthorized(pool: sqlx::PgPool) {
    let app = dlq_router(state(pool));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/admin/dlq")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_resets_dead_delivery_to_pending(pool: sqlx::PgPool) {
    let delivery_id = seed_dead(&pool).await;
    // Call the replay function path directly via the router is admin-gated; assert the
    // underlying behavior through the public helpers used by the handler.
    let replayed = platform::events::replay_dead_letter(&pool, delivery_id)
        .await
        .unwrap();
    assert!(replayed);
    let status: String = sqlx::query_scalar("select status from outbox_delivery where id = $1")
        .bind(delivery_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}
