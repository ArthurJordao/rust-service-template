use anyhow::Context;
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

fn default_access_ttl() -> i64 {
    900
}

fn default_refresh_ttl() -> i64 {
    7
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSettings {
    /// Inline RSA public-key PEM (verification). Optional if `jwt_public_key_file` is set.
    #[serde(default)]
    pub jwt_public_key_pem: String,
    /// Path to an RSA public-key PEM file; takes precedence over the inline PEM.
    #[serde(default)]
    pub jwt_public_key_file: String,
    /// Inline RSA private-key PEM (issuance). Optional if `jwt_private_key_file` is set.
    #[serde(default)]
    pub jwt_private_key_pem: String,
    /// Path to an RSA private-key PEM file; takes precedence over the inline PEM.
    #[serde(default)]
    pub jwt_private_key_file: String,
    #[serde(default = "default_access_ttl")]
    pub access_token_ttl_seconds: i64,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_token_ttl_days: i64,
    #[serde(default)]
    pub admin_emails: String,
}

/// Resolve a key PEM: read the file if a path is set, else use the inline PEM,
/// else error. Lets local dev point at a key file instead of inlining a
/// multi-line PEM in the environment / `.env`.
fn resolve_key(kind: &str, file: &str, inline: &str) -> anyhow::Result<String> {
    if !file.is_empty() {
        std::fs::read_to_string(file)
            .with_context(|| format!("reading {kind} JWT key file '{file}'"))
    } else if !inline.is_empty() {
        Ok(inline.to_string())
    } else {
        anyhow::bail!(
            "no {kind} JWT key configured (set APP__AUTH__JWT_{up}_KEY_FILE or APP__AUTH__JWT_{up}_KEY_PEM)",
            kind = kind,
            up = kind.to_uppercase()
        )
    }
}

impl AuthSettings {
    /// The RSA public-key PEM for verifying tokens (file path wins over inline).
    pub fn public_key_pem(&self) -> anyhow::Result<String> {
        resolve_key(
            "public",
            &self.jwt_public_key_file,
            &self.jwt_public_key_pem,
        )
    }

    /// The RSA private-key PEM for issuing tokens (file path wins over inline).
    pub fn private_key_pem(&self) -> anyhow::Result<String> {
        resolve_key(
            "private",
            &self.jwt_private_key_file,
            &self.jwt_private_key_pem,
        )
    }

    /// Parse the comma-separated `admin_emails` config value into a trimmed list.
    pub fn admin_email_list(&self) -> Vec<String> {
        self.admin_emails
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
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
        assert_eq!(
            s.cors_allowed_origins,
            vec!["http://localhost:5173".to_string()]
        );
    }

    fn auth(public_pem: &str, public_file: &str) -> AuthSettings {
        AuthSettings {
            jwt_public_key_pem: public_pem.into(),
            jwt_public_key_file: public_file.into(),
            jwt_private_key_pem: String::new(),
            jwt_private_key_file: String::new(),
            access_token_ttl_seconds: 900,
            refresh_token_ttl_days: 7,
            admin_emails: String::new(),
        }
    }

    #[test]
    fn key_resolution_inline_then_error() {
        assert_eq!(auth("INLINE", "").public_key_pem().unwrap(), "INLINE");
        assert!(auth("", "").public_key_pem().is_err());
    }

    #[test]
    fn key_file_takes_precedence_over_inline() {
        let dir = std::env::temp_dir();
        let path = dir.join("platform_cfg_key_test.pem");
        std::fs::write(&path, "FROM_FILE").unwrap();
        let s = auth("INLINE", path.to_str().unwrap());
        assert_eq!(s.public_key_pem().unwrap(), "FROM_FILE");
        let _ = std::fs::remove_file(&path);
    }
}
