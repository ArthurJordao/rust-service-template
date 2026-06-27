use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::ports::http::register,
        crate::ports::http::login,
        crate::ports::http::refresh,
        crate::ports::http::logout,
        crate::ports::http::list_scopes,
        crate::ports::http::list_users,
        crate::ports::http::get_user_scopes,
        crate::ports::http::set_user_scopes,
    ),
    components(schemas(
        crate::ports::dto::RegisterRequest,
        crate::ports::dto::LoginRequest,
        crate::ports::dto::AuthTokens,
        crate::ports::dto::RefreshRequest,
        crate::ports::dto::LogoutRequest,
        crate::ports::dto::UserWithScopes,
        crate::ports::dto::SetScopesRequest,
        crate::models::ScopeRow,
    )),
    tags((name = "auth"), (name = "admin"))
)]
pub struct ApiDoc;
