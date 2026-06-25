use crate::models::{NewUser, User};
use crate::ports::UserRepository;
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};

#[derive(Clone)]
pub struct PostgresUserRepository {
    pool: Db,
}

impl PostgresUserRepository {
    pub fn new(pool: Db) -> Self {
        PostgresUserRepository { pool }
    }
}

const USER_COLS: &str = "id, email, password_hash, tokens_valid_from, created_at, created_by_cid";

#[async_trait::async_trait]
impl UserRepository for PostgresUserRepository {
    async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>> {
        let row = sqlx::query_as::<_, User>(&format!(
            "select {USER_COLS} from auth_user where email = $1"
        ))
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>> {
        let row =
            sqlx::query_as::<_, User>(&format!("select {USER_COLS} from auth_user where id = $1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn list(&self) -> anyhow::Result<Vec<User>> {
        let rows =
            sqlx::query_as::<_, User>(&format!("select {USER_COLS} from auth_user order by id"))
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("select scope from user_scope where user_id = $1 order by scope")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(s,)| s).collect())
    }
}

use crate::ports::repository::{RefreshTokenRepository, StoredRefreshToken};
use chrono::{DateTime, Utc};

#[async_trait::async_trait]
impl RefreshTokenRepository for PostgresUserRepository {
    async fn store(
        &self,
        jti: &str,
        user_id: i64,
        expires_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        sqlx::query("insert into refresh_token (jti, user_id, expires_at) values ($1, $2, $3)")
            .bind(jti)
            .bind(user_id)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>> {
        let row = sqlx::query_as::<_, StoredRefreshToken>(
            "select id, jti, user_id, expires_at, revoked from refresh_token where jti = $1",
        )
        .bind(jti)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn revoke(&self, jti: &str) -> anyhow::Result<()> {
        sqlx::query("update refresh_token set revoked = true where jti = $1")
            .bind(jti)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// Insert a user, seed default scopes, and publish `user.registered` atomically.
pub async fn register_user_with_event(
    pool: &Db,
    publisher: &dyn EventPublisher,
    new: NewUser,
    default_scopes: &[&str],
    cid: &str,
) -> anyhow::Result<User> {
    let mut tx = pool.begin().await?;

    let user = sqlx::query_as::<_, User>(&format!(
        "insert into auth_user (email, password_hash, created_by_cid) \
         values ($1, $2, $3) returning {USER_COLS}"
    ))
    .bind(&new.email)
    .bind(&new.password_hash)
    .bind(cid)
    .fetch_one(&mut *tx)
    .await?;

    for scope in default_scopes {
        sqlx::query("insert into user_scope (user_id, scope) values ($1, $2)")
            .bind(user.id)
            .bind(scope)
            .execute(&mut *tx)
            .await?;
    }

    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "user.registered".into(),
                aggregate_id: user.id.to_string(),
                payload: serde_json::json!({
                    "auth_user_id": user.id,
                    "email": user.email,
                }),
                correlation_id: cid.to_string(),
            },
        )
        .await?;

    tx.commit().await?;
    Ok(user)
}
