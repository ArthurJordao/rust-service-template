use crate::db::Db;

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct DeadLetter {
    pub delivery_id: i64,
    pub subscriber_name: String,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub last_error: Option<String>,
    pub attempts: i32,
}

pub async fn list_dead_letters(pool: &Db) -> anyhow::Result<Vec<DeadLetter>> {
    let rows = sqlx::query_as::<_, DeadLetter>(
        "select d.id as delivery_id, d.subscriber_name, e.event_type, e.aggregate_id, \
                e.payload, d.last_error, d.attempts \
         from outbox_delivery d \
         join outbox_event e on e.id = d.event_id \
         where d.status = 'dead' \
         order by d.id desc",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn replay_dead_letter(pool: &Db, delivery_id: i64) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "update outbox_delivery \
         set status = 'pending', attempts = 0, last_error = null, \
             next_attempt_at = now(), updated_at = now() \
         where id = $1 and status = 'dead'",
    )
    .bind(delivery_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
