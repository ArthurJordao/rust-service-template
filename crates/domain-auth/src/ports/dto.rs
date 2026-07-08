use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginResponse {
    Authenticated {
        tokens: AuthTokens,
    },
    MfaRequired {
        purpose: String,
        mfa_token: String,
        factor_types: Vec<String>,
    },
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LogoutRequest {
    pub refresh_token: String,
    #[serde(default)]
    pub access_token: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserWithScopes {
    pub id: i64,
    pub email: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetScopesRequest {
    pub scopes: Vec<String>,
}
