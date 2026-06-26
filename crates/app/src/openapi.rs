use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::server::ServerBuilder;
use utoipa::openapi::OpenApi;
use utoipa::OpenApi as _;

/// The merged OpenAPI document for the whole API (served under `/api`).
pub fn api_doc() -> OpenApi {
    let mut doc = domain_auth::openapi::ApiDoc::openapi();
    doc.merge(domain_account::openapi::ApiDoc::openapi());
    doc.merge(platform::events::dlq_http::ApiDoc::openapi());
    doc.merge(domain_notification::openapi::ApiDoc::openapi());

    // Bearer (JWT) security scheme.
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("JWT")
                .build(),
        ),
    );

    // All paths are served under /api.
    doc.servers = Some(vec![ServerBuilder::new().url("/api").build()]);
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregated_doc_has_all_endpoints_and_schemas() {
        let doc = api_doc();
        let json = serde_json::to_string(&doc).unwrap();
        for path in [
            "/auth/login",
            "/auth/register",
            "/accounts/me",
            "/users/{id}/scopes",
            "/admin/dlq",
            "/notifications",
        ] {
            assert!(json.contains(path), "missing path {path}");
        }
        for schema in [
            "AuthTokens",
            "Account",
            "DeadLetter",
            "ReplayResponse",
            "UserWithScopes",
            "SentNotification",
        ] {
            assert!(json.contains(schema), "missing schema {schema}");
        }
        // bearer scheme + /api server present
        assert!(json.contains("bearer_auth"));
        assert!(json.contains("\"/api\""));
    }
}
