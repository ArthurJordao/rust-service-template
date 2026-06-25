use platform::auth::{AccessClaims, RevocationChecker};
use platform::db::Db;

#[derive(Clone)]
pub struct PostgresRevocationChecker {
    pool: Db,
}

impl PostgresRevocationChecker {
    pub fn new(pool: Db) -> Self {
        PostgresRevocationChecker { pool }
    }
}

#[async_trait::async_trait]
impl RevocationChecker for PostgresRevocationChecker {
    async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool> {
        // 1. Explicit denylist by jti (logout).
        let denylisted: bool =
            sqlx::query_scalar("select exists (select 1 from revoked_access_token where jti = $1)")
                .bind(&claims.jti)
                .fetch_one(&self.pool)
                .await?;
        if denylisted {
            return Ok(true);
        }

        // 2. Per-user invalidation epoch: reject tokens issued before tokens_valid_from.
        if let Some(user_id) = claims
            .sub
            .strip_prefix("user-")
            .and_then(|s| s.parse::<i64>().ok())
        {
            let valid_from: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("select tokens_valid_from from auth_user where id = $1")
                    .bind(user_id)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some(valid_from) = valid_from {
                if (claims.iat as i64) < valid_from.timestamp() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
