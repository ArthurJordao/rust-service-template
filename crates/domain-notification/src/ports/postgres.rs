use crate::models::{NewSentNotification, SentNotification};
use crate::ports::repository::SentNotificationRepository;
use platform::db::Db;

const COLS: &str =
    "id, source_event_id, template, channel, recipient, body, created_at, created_by_cid";

#[derive(Clone)]
pub struct PostgresSentNotificationRepository {
    pool: Db,
}

impl PostgresSentNotificationRepository {
    pub fn new(pool: Db) -> Self {
        PostgresSentNotificationRepository { pool }
    }
}

#[async_trait::async_trait]
impl SentNotificationRepository for PostgresSentNotificationRepository {
    async fn find_by_event_id(
        &self,
        source_event_id: i64,
    ) -> anyhow::Result<Option<SentNotification>> {
        let row = sqlx::query_as::<_, SentNotification>(&format!(
            "select {COLS} from sent_notification where source_event_id = $1"
        ))
        .bind(source_event_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn record(&self, new: NewSentNotification) -> anyhow::Result<()> {
        sqlx::query(
            "insert into sent_notification \
             (source_event_id, template, channel, recipient, body, created_by_cid) \
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(new.source_event_id)
        .bind(&new.template)
        .bind(&new.channel)
        .bind(&new.recipient)
        .bind(&new.body)
        .bind(&new.created_by_cid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<SentNotification>> {
        let rows = sqlx::query_as::<_, SentNotification>(&format!(
            "select {COLS} from sent_notification order by id desc"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}
