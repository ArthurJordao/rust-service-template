use crate::models::{ScopeRow, User};
use chrono::{DateTime, Utc};

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>>;
    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>>;
    async fn list(&self) -> anyhow::Result<Vec<User>>;
    async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>>;
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredRefreshToken {
    pub id: i64,
    pub jti: String,
    pub user_id: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
}

#[async_trait::async_trait]
pub trait ScopeRepository: Send + Sync {
    async fn list_catalog(&self) -> anyhow::Result<Vec<ScopeRow>>;
    async fn list_users_with_scopes(&self) -> anyhow::Result<Vec<(User, Vec<String>)>>;
    async fn replace_user_scopes(&self, user_id: i64, scopes: &[String]) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    async fn store(&self, jti: &str, user_id: i64, expires_at: DateTime<Utc>)
        -> anyhow::Result<()>;
    async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>>;
    async fn revoke(&self, jti: &str) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MfaFactor {
    pub id: i64,
    pub user_id: i64,
    #[sqlx(rename = "type")]
    pub factor_type: String,
    pub secret_encrypted: Vec<u8>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
pub trait MfaRepository: Send + Sync {
    async fn confirmed_factor(&self, user_id: i64) -> anyhow::Result<Option<MfaFactor>>;
    async fn get_factor(
        &self,
        user_id: i64,
        factor_type: &str,
    ) -> anyhow::Result<Option<MfaFactor>>;
    async fn upsert_unconfirmed_factor(
        &self,
        user_id: i64,
        factor_type: &str,
        secret_encrypted: &[u8],
    ) -> anyhow::Result<()>;
    async fn confirm_factor(&self, user_id: i64, factor_type: &str) -> anyhow::Result<()>;
    async fn delete_factors(&self, user_id: i64) -> anyhow::Result<()>;
    async fn record_failed_attempt(
        &self,
        factor_id: i64,
        locked_until: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()>;
    async fn reset_attempts(&self, factor_id: i64) -> anyhow::Result<()>;
    async fn store_recovery_codes(&self, user_id: i64, hashes: &[String]) -> anyhow::Result<()>;
    /// Returns true and marks used iff an unused code matching `code` exists.
    async fn consume_recovery_code(&self, user_id: i64, code: &str) -> anyhow::Result<bool>;
    async fn delete_recovery_codes(&self, user_id: i64) -> anyhow::Result<()>;
}
