use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserWithScopes {
    pub id: i64,
    pub email: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetScopesRequest {
    pub scopes: Vec<String>,
}
