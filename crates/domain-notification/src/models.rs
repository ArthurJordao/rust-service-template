use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SentNotification {
    pub id: i64,
    pub source_event_id: i64,
    pub template: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewSentNotification {
    pub source_event_id: i64,
    pub template: String,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub created_by_cid: String,
}

/// Delivery channel. Only Email today; the enum keeps the door open for more.
#[derive(Debug, Clone)]
pub enum NotificationChannel {
    Email(String),
}

impl NotificationChannel {
    /// (channel-kind string, recipient) for storage/logging.
    pub fn parts(&self) -> (&'static str, &str) {
        match self {
            NotificationChannel::Email(addr) => ("email", addr),
        }
    }
}
