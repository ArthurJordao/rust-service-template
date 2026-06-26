use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(crate::ports::http::list_notifications),
    components(schemas(crate::models::SentNotification)),
    tags((name = "notifications"))
)]
pub struct ApiDoc;
