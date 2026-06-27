use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub auth_user_id: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_cid: String,
}

#[derive(Debug, Clone)]
pub struct NewAccount {
    pub email: String,
    pub name: String,
    pub auth_user_id: i64,
}
