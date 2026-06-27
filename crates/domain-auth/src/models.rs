use serde::Serialize;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub tokens_valid_from: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ScopeRow {
    pub id: i64,
    pub name: String,
    pub description: String,
}
