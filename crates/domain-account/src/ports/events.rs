use crate::models::NewAccount;
use crate::ports::postgres::create_account_with_event;
use crate::ports::AccountRepository;
use platform::db::Db;
use platform::events::{DeliveredEvent, EventPublisher, Subscriber};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct UserRegistered {
    pub auth_user_id: i64,
    pub email: String,
}

pub struct AccountSubscriber {
    pool: Db,
    repo: Arc<dyn AccountRepository>,
    publisher: Arc<dyn EventPublisher>,
}

impl AccountSubscriber {
    pub fn new(
        pool: Db,
        repo: Arc<dyn AccountRepository>,
        publisher: Arc<dyn EventPublisher>,
    ) -> AccountSubscriber {
        AccountSubscriber { pool, repo, publisher }
    }
}

#[async_trait::async_trait]
impl Subscriber for AccountSubscriber {
    fn name(&self) -> &'static str {
        "account.on-user-registered"
    }
    fn event_type(&self) -> &'static str {
        "user.registered"
    }
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()> {
        let payload: UserRegistered = serde_json::from_value(event.payload.clone())?;

        // Fast-path idempotency check (the create is also idempotent).
        if self
            .repo
            .find_by_auth_user_id(payload.auth_user_id)
            .await?
            .is_some()
        {
            tracing::info!(uid = payload.auth_user_id, "account already exists; skipping");
            return Ok(());
        }

        create_account_with_event(
            &self.pool,
            self.publisher.as_ref(),
            NewAccount {
                email: payload.email.clone(),
                name: payload.email,
                auth_user_id: payload.auth_user_id,
            },
            &event.correlation_id,
        )
        .await?;
        Ok(())
    }
}
