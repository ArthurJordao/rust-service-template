use crate::models::Account;

#[async_trait::async_trait]
pub trait AccountRepository: Send + Sync {
    async fn list(&self) -> anyhow::Result<Vec<Account>>;
    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Account>>;
    async fn find_by_auth_user_id(&self, uid: i64) -> anyhow::Result<Option<Account>>;
}
