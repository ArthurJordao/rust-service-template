use crate::models::{Account, NewAccount};
use crate::ports::AccountRepository;
use platform::db::Db;
use platform::events::{EventPublisher, NewEvent};

#[derive(Clone)]
pub struct PostgresAccountRepository {
    pool: Db,
}

impl PostgresAccountRepository {
    pub fn new(pool: Db) -> Self {
        PostgresAccountRepository { pool }
    }
}

#[async_trait::async_trait]
impl AccountRepository for PostgresAccountRepository {
    async fn list(&self) -> anyhow::Result<Vec<Account>> {
        let rows = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account order by id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Account>> {
        let row = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_auth_user_id(&self, uid: i64) -> anyhow::Result<Option<Account>> {
        let row = sqlx::query_as::<_, Account>(
            "select id, email, name, auth_user_id, created_at, created_by_cid \
             from account where auth_user_id = $1",
        )
        .bind(uid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

/// Insert an account and publish `account.created` atomically. Idempotent on
/// `auth_user_id`: if the account already exists, returns it without inserting.
pub async fn create_account_with_event(
    pool: &Db,
    publisher: &dyn EventPublisher,
    new: NewAccount,
    cid: &str,
) -> anyhow::Result<Account> {
    let mut tx = pool.begin().await?;

    // Idempotency: return existing row if present.
    if let Some(existing) = sqlx::query_as::<_, Account>(
        "select id, email, name, auth_user_id, created_at, created_by_cid \
         from account where auth_user_id = $1",
    )
    .bind(new.auth_user_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        return Ok(existing);
    }

    let account = sqlx::query_as::<_, Account>(
        "insert into account (email, name, auth_user_id, created_by_cid) \
         values ($1, $2, $3, $4) \
         returning id, email, name, auth_user_id, created_at, created_by_cid",
    )
    .bind(&new.email)
    .bind(&new.name)
    .bind(new.auth_user_id)
    .bind(cid)
    .fetch_one(&mut *tx)
    .await?;

    publisher
        .publish(
            &mut tx,
            NewEvent {
                event_type: "account.created".into(),
                aggregate_id: account.id.to_string(),
                payload: serde_json::json!({
                    "account_id": account.id,
                    "auth_user_id": account.auth_user_id,
                    "email": account.email,
                }),
                correlation_id: cid.to_string(),
            },
        )
        .await?;

    tx.commit().await?;
    Ok(account)
}
