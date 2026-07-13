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
    state_with_revocation(
        pool,
        policy,
        Arc::new(platform::auth::NoopRevocationChecker),
    )
}

/// Test-only revocation checker that always reports the token as revoked, used to
/// prove that the MFA-enrollment auth guard consults `RevocationChecker` on the
/// normal-access-token branch.
struct AlwaysRevoked;

#[async_trait::async_trait]
impl platform::auth::RevocationChecker for AlwaysRevoked {
    async fn is_revoked(&self, _claims: &platform::auth::AccessClaims) -> anyhow::Result<bool> {
        Ok(true)
    }
}

fn state_with_admin(pool: sqlx::PgPool, policy: MfaPolicy, admin_email: &str) -> AuthState {
    let mut state = state_with(pool, policy);
    state.admin_emails = Arc::new(vec![admin_email.to_string()]);
    state
}

fn state_with_revocation(
    pool: sqlx::PgPool,
    policy: MfaPolicy,
    revocation: Arc<dyn platform::auth::RevocationChecker>,
) -> AuthState {
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
        revocation,
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
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
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
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn delete_bearer(
    app: &axum::Router,
    uri: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn get_bearer(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn current_totp_code(secret: &str) -> String {
    TotpVerifier::new("test".into())
        .current_code(secret, chrono::Utc::now())
        .unwrap()
}

/// Login (obtaining an enroll mfa-token) -> setup -> confirm. Returns the raw TOTP
/// secret so callers can compute fresh codes for a subsequent verify step.
async fn enroll(app: &axum::Router, email: &str, password: &str) -> String {
    let (_s, login) = post_json(
        app,
        "/auth/login",
        &format!(r#"{{"email":"{email}","password":"{password}"}}"#),
    )
    .await;
    let mfa_token = login["mfa_token"].as_str().unwrap().to_string();

    let (s1, setup) = post_bearer(app, "/auth/mfa/setup", &mfa_token, "{}").await;
    assert_eq!(s1, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();

    let code = current_totp_code(&secret);
    let (s2, _confirm) = post_bearer(
        app,
        "/auth/mfa/confirm",
        &mfa_token,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    secret
}

/// Login (obtaining an enroll mfa-token) -> setup -> confirm, returning the live
/// access token issued directly by `confirm` for a first-time enrollment.
async fn access_token_via_enroll(app: &axum::Router, email: &str, password: &str) -> String {
    let (_s, login) = post_json(
        app,
        "/auth/login",
        &format!(r#"{{"email":"{email}","password":"{password}"}}"#),
    )
    .await;
    let mfa_token = login["mfa_token"].as_str().unwrap().to_string();

    let (s1, setup) = post_bearer(app, "/auth/mfa/setup", &mfa_token, "{}").await;
    assert_eq!(s1, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();

    let code = current_totp_code(&secret);
    let (s2, confirm) = post_bearer(
        app,
        "/auth/mfa/confirm",
        &mfa_token,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    confirm["tokens"]["access_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Self-enroll via setup + confirm with an access token (not an mfa_token).
/// Returns the raw TOTP secret.
async fn self_enroll(app: &axum::Router, access: &str) -> String {
    let (s1, setup) = post_bearer(app, "/auth/mfa/setup", access, "{}").await;
    assert_eq!(s1, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();

    let code = current_totp_code(&secret);
    let (s2, _confirm) = post_bearer(
        app,
        "/auth/mfa/confirm",
        access,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    secret
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

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_access_token_rejected_on_setup(pool: sqlx::PgPool) {
    let app = router(state_with_revocation(
        pool.clone(),
        MfaPolicy::Optional,
        Arc::new(AlwaysRevoked),
    ));
    register(&app, "a@b.c", "pw").await;
    let (status, login) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    assert_eq!(login["status"], "authenticated");
    let access_token = login["tokens"]["access_token"].as_str().unwrap();

    // A structurally valid, unexpired access token whose jti/user the RevocationChecker
    // reports as revoked must still be rejected — mirroring the `Authenticated` extractor.
    let (setup_status, _) = post_bearer(&app, "/auth/mfa/setup", access_token, "{}").await;
    assert_eq!(setup_status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn refresh_token_rejected_on_setup(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Optional));
    register(&app, "a@b.c", "pw").await;
    let (status, login) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(status, 200);
    let refresh_token = login["tokens"]["refresh_token"].as_str().unwrap();

    // A refresh token is the wrong purpose for MFA setup: it isn't a live access
    // token and its `token_type` isn't in the enroll-token allow-list.
    let (setup_status, _) = post_bearer(&app, "/auth/mfa/setup", refresh_token, "{}").await;
    assert_eq!(setup_status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn mfa_pending_token_rejected_on_setup(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    // First login enrolls (policy=Required, no factor yet) — set up + confirm a factor
    // so the *second* login issues an `mfa_pending` (verify) challenge token instead.
    let (_s, first_login) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let enroll_token = first_login["mfa_token"].as_str().unwrap().to_string();
    let (s1, setup) = post_bearer(&app, "/auth/mfa/setup", &enroll_token, "{}").await;
    assert_eq!(s1, 200);
    let secret = setup["secret"].as_str().unwrap().to_string();
    let code = current_totp_code(&secret);
    let (s2, _confirm) = post_bearer(
        &app,
        "/auth/mfa/confirm",
        &enroll_token,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(s2, 200);

    let (_s3, second_login) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(second_login["status"], "mfa_required");
    assert_eq!(second_login["purpose"], "verify");
    let pending_token = second_login["mfa_token"].as_str().unwrap();

    // setup only allows `mfa_enroll` (or a live access token), not `mfa_pending`.
    let (setup_status, _) = post_bearer(&app, "/auth/mfa/setup", pending_token, "{}").await;
    assert_eq!(setup_status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_completes_login_and_wrong_code_locks_out(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let secret = enroll(&app, "a@b.c", "pw").await; // helper: login->setup->confirm, returns secret

    // subsequent login now requires verify
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    assert_eq!(login["purpose"], "verify");
    let pending = login["mfa_token"].as_str().unwrap().to_string();

    // wrong code fails
    let (bad, _) = post_bearer(&app, "/auth/mfa/verify", &pending, r#"{"code":"000000"}"#).await;
    assert_eq!(bad, StatusCode::UNAUTHORIZED);

    // correct code succeeds
    let code = current_totp_code(&secret);
    let (ok, tokens) = post_bearer(
        &app,
        "/auth/mfa/verify",
        &pending,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    assert!(tokens["access_token"].as_str().unwrap().len() > 10);
}

#[sqlx::test(migrations = "../../migrations")]
async fn verify_with_recovery_code_is_single_use(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    enroll(&app, "a@b.c", "pw").await;

    // Grab a recovery code directly from the confirm response by re-running the
    // enroll flow's confirm step is not possible (codes only shown once at confirm
    // time and enroll() discards them), so fetch a fresh set via the DB-backed repo.
    let recovery_code = {
        let repo = domain_auth::ports::postgres::PostgresUserRepository::new(pool.clone());
        use domain_auth::ports::MfaRepository;
        let codes = domain_auth::auth::recovery::generate_recovery_codes();
        let hashes: Vec<String> = codes
            .iter()
            .map(|c| domain_auth::auth::recovery::hash_recovery_code(c).unwrap())
            .collect();
        let user = {
            use domain_auth::ports::UserRepository;
            repo.find_by_email("a@b.c").await.unwrap().unwrap()
        };
        repo.store_recovery_codes(user.id, &hashes).await.unwrap();
        codes[0].clone()
    };

    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let pending = login["mfa_token"].as_str().unwrap().to_string();

    let (ok, _tokens) = post_bearer(
        &app,
        "/auth/mfa/verify",
        &pending,
        &format!(r#"{{"code":"{recovery_code}"}}"#),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);

    // second login + reuse of the same recovery code must fail
    let (_s2, login2) =
        post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let pending2 = login2["mfa_token"].as_str().unwrap().to_string();
    let (reused, _) = post_bearer(
        &app,
        "/auth/mfa/verify",
        &pending2,
        &format!(r#"{{"code":"{recovery_code}"}}"#),
    )
    .await;
    assert_eq!(reused, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn access_token_rejected_on_verify(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let secret = enroll(&app, "a@b.c", "pw").await; // helper: login->setup->confirm, returns secret

    // Complete a full verify to obtain a live access token.
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let pending = login["mfa_token"].as_str().unwrap().to_string();
    let code = current_totp_code(&secret);
    let (ok, tokens) = post_bearer(
        &app,
        "/auth/mfa/verify",
        &pending,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(ok, StatusCode::OK);
    let access_token = tokens["access_token"].as_str().unwrap();

    // A live access token is not an `mfa_pending` token: verify must reject it even
    // though `mfa_user_id`'s access-token branch would otherwise accept it.
    let (verify_status, _) = post_bearer(
        &app,
        "/auth/mfa/verify",
        access_token,
        &format!(r#"{{"code":"{code}"}}"#),
    )
    .await;
    assert_eq!(verify_status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_reset_clears_factor_and_emits_event(pool: sqlx::PgPool) {
    let app = router(state_with_admin(
        pool.clone(),
        MfaPolicy::Required,
        "admin@x.y",
    ));
    register(&app, "admin@x.y", "pw").await;
    // enroll->confirm gives the admin both a confirmed factor and a live (admin-scoped)
    // access token in one shot, since confirm issues tokens for a first-time enrollment.
    let admin_token = access_token_via_enroll(&app, "admin@x.y", "pw").await;
    let uid: i64 = sqlx::query_scalar("select id from auth_user where email='admin@x.y'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let (s, _) = post_bearer(
        &app,
        &format!("/admin/users/{uid}/mfa/reset"),
        &admin_token,
        "{}",
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let cnt: i64 = sqlx::query_scalar("select count(*) from auth_mfa_factor where user_id=$1")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cnt, 0);
    let ev: i64 =
        sqlx::query_scalar("select count(*) from outbox_event where event_type='user.mfa_reset'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ev, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn self_disable_rejected_when_required(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Required));
    register(&app, "a@b.c", "pw").await;
    let token = access_token_via_enroll(&app, "a@b.c", "pw").await;
    let (s, _) = delete_bearer(&app, "/auth/mfa", &token).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn mfa_status_reports_enabled_after_enroll(pool: sqlx::PgPool) {
    let app = router(state_with(pool.clone(), MfaPolicy::Optional));
    register(&app, "a@b.c", "pw").await;
    // before enrolling: authenticated login returns a session token
    let (_s, login) = post_json(&app, "/auth/login", r#"{"email":"a@b.c","password":"pw"}"#).await;
    let access = login["tokens"]["access_token"]
        .as_str()
        .unwrap()
        .to_string();

    let (s1, before) = get_bearer(&app, "/auth/mfa", &access).await;
    assert_eq!(s1, 200);
    assert_eq!(before["enabled"], false);
    assert_eq!(before["policy"], "optional");

    // self-enroll via the existing setup/confirm helpers, then re-check
    let secret = self_enroll(&app, &access).await; // helper: setup+confirm with access token
    let _ = secret;
    let (_s2, after) = get_bearer(&app, "/auth/mfa", &access).await;
    assert_eq!(after["enabled"], true);
}
