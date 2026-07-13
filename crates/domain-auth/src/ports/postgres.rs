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

use crate::models::ScopeRow;
use crate::ports::repository::ScopeRepository;

#[async_trait::async_trait]
impl ScopeRepository for PostgresUserRepository {
    async fn list_catalog(&self) -> anyhow::Result<Vec<ScopeRow>> {
        let rows =
            sqlx::query_as::<_, ScopeRow>("select id, name, description from scope order by name")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_users_with_scopes(&self) -> anyhow::Result<Vec<(User, Vec<String>)>> {
        let users =
            sqlx::query_as::<_, User>(&format!("select {USER_COLS} from auth_user order by id"))
                .fetch_all(&self.pool)
                .await?;
        let mut out = Vec::with_capacity(users.len());
        for user in users {
            let scopes: Vec<(String,)> =
                sqlx::query_as("select scope from user_scope where user_id = $1 order by scope")
                    .bind(user.id)
                    .fetch_all(&self.pool)
                    .await?;
            out.push((user, scopes.into_iter().map(|(s,)| s).collect()));
        }
        Ok(out)
    }

    async fn replace_user_scopes(&self, user_id: i64, scopes: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("delete from user_scope where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for scope in scopes {
            sqlx::query("insert into user_scope (user_id, scope) values ($1, $2)")
                .bind(user_id)
                .bind(scope)
                .execute(&mut *tx)
                .await?;
        }
        // Invalidate the user's existing access tokens (per-user revocation epoch).
        sqlx::query("update auth_user set tokens_valid_from = now() where id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

use crate::ports::repository::{MfaFactor, MfaRepository};

#[async_trait::async_trait]
impl MfaRepository for PostgresUserRepository {
    async fn confirmed_factor(&self, user_id: i64) -> anyhow::Result<Option<MfaFactor>> {
        let f = sqlx::query_as::<_, MfaFactor>(
            "select id, user_id, type, secret_encrypted, confirmed_at, failed_attempts, locked_until \
             from auth_mfa_factor where user_id = $1 and confirmed_at is not null",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(f)
    }

    async fn get_factor(
        &self,
        user_id: i64,
        factor_type: &str,
    ) -> anyhow::Result<Option<MfaFactor>> {
        let f = sqlx::query_as::<_, MfaFactor>(
            "select id, user_id, type, secret_encrypted, confirmed_at, failed_attempts, locked_until \
             from auth_mfa_factor where user_id = $1 and type = $2",
        )
        .bind(user_id)
        .bind(factor_type)
        .fetch_optional(&self.pool)
        .await?;
        Ok(f)
    }

    async fn upsert_unconfirmed_factor(
        &self,
        user_id: i64,
        factor_type: &str,
        secret_encrypted: &[u8],
    ) -> anyhow::Result<()> {
        sqlx::query(
            "insert into auth_mfa_factor (user_id, type, secret_encrypted) values ($1, $2, $3) \
             on conflict (user_id, type) do update set secret_encrypted = excluded.secret_encrypted, \
                 confirmed_at = null, failed_attempts = 0, locked_until = null",
        )
        .bind(user_id)
        .bind(factor_type)
        .bind(secret_encrypted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn confirm_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<()> {
        sqlx::query(
            "update auth_mfa_factor set confirmed_at = now() where user_id = $1 and type = $2",
        )
        .bind(user_id)
        .bind(factor_type)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_factors(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("delete from auth_mfa_factor where user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn record_failed_attempt(
        &self,
        factor_id: i64,
        locked_until: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "update auth_mfa_factor set failed_attempts = failed_attempts + 1, locked_until = $2 where id = $1",
        )
        .bind(factor_id)
        .bind(locked_until)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn reset_attempts(&self, factor_id: i64) -> anyhow::Result<()> {
        sqlx::query(
            "update auth_mfa_factor set failed_attempts = 0, locked_until = null where id = $1",
        )
        .bind(factor_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn store_recovery_codes(&self, user_id: i64, hashes: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("delete from auth_mfa_recovery_code where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for h in hashes {
            sqlx::query("insert into auth_mfa_recovery_code (user_id, code_hash) values ($1, $2)")
                .bind(user_id)
                .bind(h)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn consume_recovery_code(&self, user_id: i64, code: &str) -> anyhow::Result<bool> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "select id, code_hash from auth_mfa_recovery_code where user_id = $1 and used_at is null",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        for (id, hash) in rows {
            if crate::auth::recovery::verify_recovery_code(&hash, code) {
                let res = sqlx::query(
                    "update auth_mfa_recovery_code set used_at = now() where id = $1 and used_at is null",
                )
                .bind(id)
                .execute(&self.pool)
                .await?;
                if res.rows_affected() == 1 {
                    return Ok(true);
                }
                // else: a concurrent request already consumed this code — keep scanning.
            }
        }
        Ok(false)
    }

    async fn delete_recovery_codes(&self, user_id: i64) -> anyhow::Result<()> {
        sqlx::query("delete from auth_mfa_recovery_code where user_id = $1")
            .bind(user_id)
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
