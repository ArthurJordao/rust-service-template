use crate::models::User;
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
pub trait RefreshTokenRepository: Send + Sync {
    async fn store(&self, jti: &str, user_id: i64, expires_at: DateTime<Utc>)
        -> anyhow::Result<()>;
    async fn find_by_jti(&self, jti: &str) -> anyhow::Result<Option<StoredRefreshToken>>;
    async fn revoke(&self, jti: &str) -> anyhow::Result<()>;
}
