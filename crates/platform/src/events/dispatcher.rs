use crate::db::Db;
use crate::events::{DeliveredEvent, SubscriberRegistry};
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

pub struct DispatcherConfig {
    pub max_attempts: i32,
    pub batch_size: i64,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig {
            max_attempts: 5,
            batch_size: 50,
        }
    }
}

#[derive(sqlx::FromRow)]
struct DueRow {
    delivery_id: i64,
    subscriber_name: String,
    attempts: i32,
    event_id: i64,
    event_type: String,
    aggregate_id: String,
    payload: serde_json::Value,
    correlation_id: String,
}

/// Process one batch of due deliveries. Returns the number attempted.
pub async fn dispatch_once(
    pool: &Db,
    registry: &SubscriberRegistry,
    config: &DispatcherConfig,
) -> anyhow::Result<usize> {
    let rows: Vec<DueRow> = sqlx::query_as(
        "select d.id as delivery_id, d.subscriber_name, d.attempts, \
                e.id as event_id, e.event_type, e.aggregate_id, e.payload, e.correlation_id \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.status = 'pending' and d.next_attempt_at <= now() \
         order by d.id \
         limit $1",
    )
    .bind(config.batch_size)
    .fetch_all(pool)
    .await?;

    let attempted = rows.len();

    for row in rows {
        let delivered = DeliveredEvent {
            event_id: row.event_id,
            event_type: row.event_type,
            aggregate_id: row.aggregate_id,
            payload: row.payload,
            correlation_id: row.correlation_id.clone(),
        };

        let Some(subscriber) = registry.find(&row.subscriber_name) else {
            // No handler registered under this name (e.g. removed subscriber).
            tracing::warn!(name = %row.subscriber_name, "no subscriber registered; skipping");
            continue;
        };

        let span = tracing::info_span!(
            "event.handle",
            cid = %row.correlation_id,
            subscriber = subscriber.name(),
            event_type = %delivered.event_type,
        );

        let result = subscriber.handle(&delivered).instrument(span).await;

        match result {
            Ok(()) => {
                sqlx::query(
                    "update outbox_delivery set status = 'delivered', updated_at = now() \
                     where id = $1",
                )
                .bind(row.delivery_id)
                .execute(pool)
                .await?;
                tracing::info!(
                    delivery_id = row.delivery_id,
                    subscriber = %row.subscriber_name,
                    event_type = %delivered.event_type,
                    "delivery delivered"
                );
            }
            Err(e) => {
                mark_failure(
                    pool,
                    row.delivery_id,
                    &row.subscriber_name,
                    &delivered.event_type,
                    row.attempts,
                    config.max_attempts,
                    &e,
                )
                .await?;
            }
        }
    }

    Ok(attempted)
}

async fn mark_failure(
    pool: &Db,
    delivery_id: i64,
    subscriber: &str,
    event_type: &str,
    attempts: i32,
    max_attempts: i32,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let next_attempts = attempts + 1;
    if next_attempts >= max_attempts {
        sqlx::query(
            "update outbox_delivery \
             set status = 'dead', attempts = $2, last_error = $3, updated_at = now() \
             where id = $1",
        )
        .bind(delivery_id)
        .bind(next_attempts)
        .bind(err.to_string())
        .execute(pool)
        .await?;
        tracing::error!(
            delivery_id,
            subscriber = %subscriber,
            event_type = %event_type,
            error = %err,
            "delivery dead-lettered"
        );
    } else {
        let backoff_secs = (2_i64.pow(next_attempts as u32)).min(300);
        sqlx::query(
            "update outbox_delivery \
             set attempts = $2, last_error = $3, \
                 next_attempt_at = now() + ($4 || ' seconds')::interval, updated_at = now() \
             where id = $1",
        )
        .bind(delivery_id)
        .bind(next_attempts)
        .bind(err.to_string())
        .bind(backoff_secs.to_string())
        .execute(pool)
        .await?;
        tracing::warn!(
            delivery_id,
            subscriber = %subscriber,
            event_type = %event_type,
            attempt = next_attempts,
            error = %err,
            "delivery failed; will retry"
        );
    }
    Ok(())
}

/// Long-running dispatcher: drain due deliveries, sleep, repeat.
pub async fn run_dispatcher(
    pool: Db,
    registry: Arc<SubscriberRegistry>,
    config: DispatcherConfig,
    poll_interval: Duration,
) {
    tracing::info!("outbox dispatcher started");
    loop {
        match dispatch_once(&pool, &registry, &config).await {
            Ok(n) if n > 0 => tracing::debug!(attempted = n, "dispatched batch"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "dispatch batch failed"),
        }
        tokio::time::sleep(poll_interval).await;
    }
}
