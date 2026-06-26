use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::ports::http::list_accounts,
        crate::ports::http::account_me,
        crate::ports::http::get_account,
    ),
    components(schemas(crate::models::Account)),
    tags((name = "accounts"))
)]
pub struct ApiDoc;
