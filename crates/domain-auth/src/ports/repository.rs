use crate::models::User;

#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_email(&self, email: &str) -> anyhow::Result<Option<User>>;
    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<User>>;
    async fn list(&self) -> anyhow::Result<Vec<User>>;
    async fn scope_names(&self, user_id: i64) -> anyhow::Result<Vec<String>>;
}
