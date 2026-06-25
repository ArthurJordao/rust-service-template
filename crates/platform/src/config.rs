use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    pub port: u16,
    pub environment: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
    pub auto_migrate: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSettings {
    pub jwt_public_key_pem: String,
}

fn default_cors() -> Vec<String> {
    vec!["http://localhost:5173".to_string()]
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub auth: AuthSettings,
    #[serde(default = "default_cors")]
    pub cors_allowed_origins: Vec<String>,
}

impl Settings {
    /// Load settings from environment variables prefixed `APP__`,
    /// nested with `__` (e.g. `APP__SERVER__PORT`).
    /// Comma-separated lists (cors origins) are split via the list parser.
    pub fn load() -> Result<Settings, config::ConfigError> {
        config::Config::builder()
            .add_source(
                config::Environment::with_prefix("APP")
                    .separator("__")
                    .list_separator(",")
                    .with_list_parse_key("cors_allowed_origins")
                    .try_parsing(true),
            )
            .build()?
            .try_deserialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_settings_from_env() {
        std::env::set_var("APP__SERVER__PORT", "9999");
        std::env::set_var("APP__SERVER__ENVIRONMENT", "test");
        std::env::set_var("APP__DATABASE__URL", "postgres://localhost/x");
        std::env::set_var("APP__DATABASE__MAX_CONNECTIONS", "3");
        std::env::set_var("APP__DATABASE__AUTO_MIGRATE", "true");
        std::env::set_var("APP__AUTH__JWT_PUBLIC_KEY_PEM", "PEM");
        std::env::set_var("APP__CORS_ALLOWED_ORIGINS", "http://localhost:5173");

        let s = Settings::load().expect("settings load");
        assert_eq!(s.server.port, 9999);
        assert_eq!(s.database.max_connections, 3);
        assert!(s.database.auto_migrate);
        assert_eq!(s.cors_allowed_origins, vec!["http://localhost:5173".to_string()]);
    }
}
