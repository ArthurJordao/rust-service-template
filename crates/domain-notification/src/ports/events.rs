use crate::models::{NewSentNotification, NotificationChannel};
use crate::ports::notifier::Notifier;
use crate::ports::repository::SentNotificationRepository;
use crate::ports::templates::Templates;
use platform::events::{DeliveredEvent, Subscriber};
use serde::Deserialize;
use std::sync::Arc;

/// Local view of the `account.created` payload (no dependency on domain-account).
#[derive(Debug, Deserialize)]
struct AccountCreated {
    account_id: i64,
    #[allow(dead_code)]
    auth_user_id: i64,
    email: String,
}

pub struct NotificationSubscriber {
    repo: Arc<dyn SentNotificationRepository>,
    notifier: Arc<dyn Notifier>,
    templates: Arc<Templates>,
}

impl NotificationSubscriber {
    pub fn new(
        repo: Arc<dyn SentNotificationRepository>,
        notifier: Arc<dyn Notifier>,
        templates: Arc<Templates>,
    ) -> NotificationSubscriber {
        NotificationSubscriber {
            repo,
            notifier,
            templates,
        }
    }
}

#[async_trait::async_trait]
impl Subscriber for NotificationSubscriber {
    fn name(&self) -> &'static str {
        "notification.on-account-created"
    }
    fn event_type(&self) -> &'static str {
        "account.created"
    }
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()> {
        // Idempotency: at-least-once delivery, so skip if already sent for this event.
        if self.repo.find_by_event_id(event.event_id).await?.is_some() {
            tracing::info!(
                event_id = event.event_id,
                "notification already sent; skipping"
            );
            return Ok(());
        }
        let payload: AccountCreated = serde_json::from_value(event.payload.clone())?;

        // Dev/test failure hook (mirrors the Haskell @fail.com): forces a DLQ path.
        if payload.email.ends_with("@fail.test") {
            anyhow::bail!("simulated notification failure for {}", payload.email);
        }

        let body = self.templates.render(
            "welcome",
            &serde_json::json!({ "email": payload.email, "account_id": payload.account_id }),
        )?;
        let subject = "Welcome";
        let channel = NotificationChannel::Email(payload.email.clone());
        self.notifier.send(&channel, subject, &body).await?;

        let (kind, recipient) = channel.parts();
        self.repo
            .record(NewSentNotification {
                source_event_id: event.event_id,
                template: "welcome".into(),
                subject: subject.into(),
                channel: kind.into(),
                recipient: recipient.into(),
                body,
                created_by_cid: event.correlation_id.clone(),
            })
            .await?;
        Ok(())
    }
}
