use crate::events::{NewEvent, Routes};

#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    /// Persist an event and a pending delivery row per interested subscriber,
    /// using the caller's transaction so it commits atomically with state.
    async fn publish(
        &self,
        conn: &mut sqlx::PgConnection,
        event: NewEvent,
    ) -> anyhow::Result<i64>;
}

pub struct OutboxPublisher {
    routes: Routes,
}

impl OutboxPublisher {
    pub fn new(routes: Routes) -> OutboxPublisher {
        OutboxPublisher { routes }
    }
}

#[async_trait::async_trait]
impl EventPublisher for OutboxPublisher {
    async fn publish(
        &self,
        conn: &mut sqlx::PgConnection,
        event: NewEvent,
    ) -> anyhow::Result<i64> {
        let event_id: i64 = sqlx::query_scalar(
            "insert into outbox_event (event_type, aggregate_id, payload, correlation_id) \
             values ($1, $2, $3, $4) returning id",
        )
        .bind(&event.event_type)
        .bind(&event.aggregate_id)
        .bind(&event.payload)
        .bind(&event.correlation_id)
        .fetch_one(&mut *conn)
        .await?;

        for name in self.routes.names_for(&event.event_type) {
            sqlx::query(
                "insert into outbox_delivery (event_id, subscriber_name) values ($1, $2)",
            )
            .bind(event_id)
            .bind(&name)
            .execute(&mut *conn)
            .await?;
        }

        Ok(event_id)
    }
}
