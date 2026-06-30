use crate::db::Db;
use crate::events::{DeliveredEvent, Subscriber, SubscriberRegistry};
use futures::stream::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

#[derive(Debug, Clone)]
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

/// How the reaper reclaims rows stuck in `processing` (worker crashed mid-flight).
#[derive(Debug, Clone)]
pub struct ReaperConfig {
    /// A `processing` row older than this is assumed orphaned and returned to the
    /// queue. Configurable; default 5 min. Raise only as a deliberate exception —
    /// a handler routinely exceeding it is the real smell.
    pub visibility_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ReaperConfig {
    fn default() -> Self {
        ReaperConfig {
            visibility_timeout: Duration::from_secs(300),
            poll_interval: Duration::from_secs(30),
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct DueRow {
    pub delivery_id: i64,
    pub subscriber_name: String,
    pub attempts: i32,
    pub event_id: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}

/// Claim up to `batch_size` due deliveries for one subscriber, flipping them to
/// `processing` in a single short transaction. `FOR UPDATE … SKIP LOCKED` makes a
/// concurrent claimer skip these rows and take the next free ones — no double-grab.
/// The lock is released on commit, BEFORE any handler runs.
pub async fn claim_batch(
    pool: &Db,
    subscriber_name: &str,
    batch_size: i64,
) -> anyhow::Result<Vec<DueRow>> {
    let mut tx = pool.begin().await?;
    let rows: Vec<DueRow> = sqlx::query_as(
        "select d.id as delivery_id, d.subscriber_name, d.attempts, \
                e.id as event_id, e.event_type, e.aggregate_id, e.payload, e.correlation_id \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.subscriber_name = $1 and d.status = 'pending' and d.next_attempt_at <= now() \
         order by d.id \
         limit $2 \
         for update of d skip locked",
    )
    .bind(subscriber_name)
    .bind(batch_size)
    .fetch_all(&mut *tx)
    .await?;

    if !rows.is_empty() {
        let ids: Vec<i64> = rows.iter().map(|r| r.delivery_id).collect();
        sqlx::query(
            "update outbox_delivery set status = 'processing', updated_at = now() \
             where id = any($1)",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(rows)
}

async fn ack_delivered(pool: &Db, delivery_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "update outbox_delivery set status = 'delivered', updated_at = now() where id = $1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Run one claim-and-handle cycle for a single subscriber. Claims up to the
/// subscriber's `batch_size`, handles the batch under `buffer_unordered(concurrency)`
/// (concurrency 1 = strictly serial, in id order), and acks each row. Handlers run
/// with NO transaction open. Returns the number of deliveries attempted.
pub async fn dispatch_subscriber_once(
    pool: &Db,
    subscriber: &dyn Subscriber,
    config: &DispatcherConfig,
) -> anyhow::Result<usize> {
    let cfg = subscriber.consumer_config();
    let rows = claim_batch(pool, subscriber.name(), cfg.batch_size).await?;
    let attempted = rows.len();

    futures::stream::iter(rows)
        .map(|row| async move {
            let delivered = DeliveredEvent {
                event_id: row.event_id,
                event_type: row.event_type,
                aggregate_id: row.aggregate_id,
                payload: row.payload,
                correlation_id: row.correlation_id.clone(),
            };

            let span = tracing::info_span!(
                "event.handle",
                cid = %row.correlation_id,
                subscriber = subscriber.name(),
                event_type = %delivered.event_type,
            );

            match subscriber.handle(&delivered).instrument(span).await {
                Ok(()) => {
                    if let Err(e) = ack_delivered(pool, row.delivery_id).await {
                        tracing::error!(delivery_id = row.delivery_id, error = %e, "ack delivered failed");
                    } else {
                        tracing::info!(
                            delivery_id = row.delivery_id,
                            subscriber = %row.subscriber_name,
                            event_type = %delivered.event_type,
                            "delivery delivered"
                        );
                    }
                }
                Err(e) => {
                    if let Err(e2) = mark_failure(
                        pool,
                        row.delivery_id,
                        &row.subscriber_name,
                        &delivered.event_type,
                        row.attempts,
                        config.max_attempts,
                        &e,
                    )
                    .await
                    {
                        tracing::error!(delivery_id = row.delivery_id, error = %e2, "mark_failure failed");
                    }
                }
            }
        })
        .buffer_unordered(cfg.concurrency.max(1))
        .collect::<Vec<()>>()
        .await;

    Ok(attempted)
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
             set status = 'pending', attempts = $2, last_error = $3, \
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

/// Long-running consumer loop for ONE subscriber: claim → handle → ack, then sleep
/// `poll_interval`. Drains immediately (no sleep) when a full batch was claimed.
pub async fn run_subscriber_loop(
    pool: Db,
    subscriber: Arc<dyn Subscriber>,
    config: DispatcherConfig,
) {
    let cfg = subscriber.consumer_config();
    let batch_size = cfg.batch_size as usize;
    tracing::info!(subscriber = subscriber.name(), "consumer loop started");
    loop {
        match dispatch_subscriber_once(&pool, subscriber.as_ref(), &config).await {
            Ok(n) if n >= batch_size && batch_size > 0 => continue,
            Ok(_) => {}
            Err(e) => {
                tracing::error!(subscriber = subscriber.name(), error = %e, "dispatch cycle failed")
            }
        }
        tokio::time::sleep(cfg.poll_interval).await;
    }
}

/// Spawn one consumer loop per registered subscriber, plus the reaper. Returns if
/// any task exits (loops never return normally, so that signals a problem).
pub async fn run_consumers(
    pool: Db,
    registry: Arc<SubscriberRegistry>,
    dispatcher: DispatcherConfig,
    reaper: ReaperConfig,
) {
    let max_attempts = dispatcher.max_attempts;
    let mut set = tokio::task::JoinSet::new();
    for sub in registry.subscribers() {
        set.spawn(run_subscriber_loop(pool.clone(), sub, dispatcher.clone()));
    }
    set.spawn(run_reaper(pool, reaper, max_attempts));
    if set.join_next().await.is_some() {
        tracing::error!("a consumer task exited unexpectedly");
    }
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

/// Reclaim deliveries stuck in `processing` past the visibility timeout (worker
/// crashed mid-flight). Bumps attempts so a row that reliably crashes the worker is
/// still eventually dead-lettered. Redelivery is safe because handlers are idempotent.
pub async fn reap_stale(
    pool: &Db,
    visibility_timeout: Duration,
    max_attempts: i32,
) -> anyhow::Result<u64> {
    let secs = visibility_timeout.as_secs() as i64;
    let result = sqlx::query(
        "update outbox_delivery \
         set attempts = attempts + 1, \
             status = case when attempts + 1 >= $2 then 'dead' else 'pending' end, \
             last_error = 'reclaimed: processing timed out', \
             next_attempt_at = now(), updated_at = now() \
         where status = 'processing' \
           and updated_at < now() - ($1 || ' seconds')::interval",
    )
    .bind(secs.to_string())
    .bind(max_attempts)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Long-running reaper: sweep stale `processing` rows, sleep, repeat.
pub async fn run_reaper(pool: Db, config: ReaperConfig, max_attempts: i32) {
    tracing::info!("outbox reaper started");
    loop {
        match reap_stale(&pool, config.visibility_timeout, max_attempts).await {
            Ok(n) if n > 0 => tracing::warn!(reclaimed = n, "reaped stale processing deliveries"),
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "reaper sweep failed"),
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}
