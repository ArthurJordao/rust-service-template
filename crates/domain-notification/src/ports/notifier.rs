use crate::models::NotificationChannel;

/// Dispatches a rendered notification through some channel. Real providers
/// (SMTP/SES/Resend) implement this later; the dev impl just logs.
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn send(
        &self,
        channel: &NotificationChannel,
        subject: &str,
        body: &str,
    ) -> anyhow::Result<()>;
}

/// Dev notifier: logs the dispatch (cid-tagged via the active span). No real send.
pub struct LogNotifier;

#[async_trait::async_trait]
impl Notifier for LogNotifier {
    async fn send(
        &self,
        channel: &NotificationChannel,
        subject: &str,
        _body: &str,
    ) -> anyhow::Result<()> {
        let (kind, recipient) = channel.parts();
        tracing::info!(channel = kind, recipient = %recipient, subject = %subject, "notification dispatched");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NotificationChannel;

    #[tokio::test]
    async fn log_notifier_sends_ok() {
        let n = LogNotifier;
        let ch = NotificationChannel::Email("a@b.c".into());
        assert!(n.send(&ch, "Welcome", "body").await.is_ok());
    }
}
