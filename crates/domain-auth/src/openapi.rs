use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::ports::http::register,
        crate::ports::http::login,
        crate::ports::http::refresh,
        crate::ports::http::logout,
        crate::ports::http::mfa_setup,
        crate::ports::http::mfa_confirm,
        crate::ports::http::mfa_verify,
        crate::ports::http::mfa_regen_recovery,
        crate::ports::http::mfa_status,
        crate::ports::http::mfa_self_disable,
        crate::ports::http::admin_mfa_reset,
        crate::ports::http::list_scopes,
        crate::ports::http::list_users,
        crate::ports::http::get_user_scopes,
        crate::ports::http::set_user_scopes,
    ),
    components(schemas(
        crate::ports::dto::RegisterRequest,
        crate::ports::dto::LoginRequest,
        crate::ports::dto::AccessTokenResponse,
        crate::ports::dto::LoginResponse,
        crate::ports::dto::LogoutRequest,
        crate::ports::dto::UserWithScopes,
        crate::ports::dto::SetScopesRequest,
        crate::ports::dto::MfaSetupResponse,
        crate::ports::dto::MfaConfirmRequest,
        crate::ports::dto::MfaConfirmResponse,
        crate::ports::dto::MfaStatusResponse,
        crate::ports::dto::MfaVerifyRequest,
        crate::models::ScopeRow,
    )),
    tags((name = "auth"), (name = "admin"))
)]
pub struct ApiDoc;
