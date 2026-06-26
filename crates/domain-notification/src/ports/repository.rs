use crate::models::SentNotification;

#[async_trait::async_trait]
pub trait SentNotificationRepository: Send + Sync {
    async fn find_by_event_id(
        &self,
        source_event_id: i64,
    ) -> anyhow::Result<Option<SentNotification>>;
    async fn record(&self, new: crate::models::NewSentNotification) -> anyhow::Result<()>;
    async fn list(&self) -> anyhow::Result<Vec<SentNotification>>;
}
