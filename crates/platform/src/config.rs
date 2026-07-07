use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MfaPolicy {
    Off,
    Optional,
    #[default]
    Required,
}

impl std::str::FromStr for MfaPolicy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<MfaPolicy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(MfaPolicy::Off),
            "optional" => Ok(MfaPolicy::Optional),
            "required" => Ok(MfaPolicy::Required),
            other => anyhow::bail!("invalid mfa_policy '{other}' (off|optional|required)"),
        }
    }
}

fn default_mfa_policy() -> String {
    "required".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    pub port: u16,
    pub environment: String,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default = "default_auth_rate_limit_per_minute")]
    pub auth_rate_limit_per_minute: u32,
    #[serde(default = "default_auth_rate_limit_burst")]
    pub auth_rate_limit_burst: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
    pub auto_migrate: bool,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_acquire_timeout_seconds")]
    pub acquire_timeout_seconds: u64,
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_max_lifetime_seconds")]
    pub max_lifetime_seconds: u64,
    #[serde(default = "default_statement_timeout_ms")]
    pub statement_timeout_ms: u64,
    #[serde(default = "default_lock_timeout_ms")]
    pub lock_timeout_ms: u64,
}

fn default_request_timeout_seconds() -> u64 {
    30
}
fn default_max_body_bytes() -> usize {
    1_048_576
}
fn default_auth_rate_limit_per_minute() -> u32 {
    10
}
fn default_auth_rate_limit_burst() -> u32 {
    5
}
fn default_min_connections() -> u32 {
    1
}
fn default_acquire_timeout_seconds() -> u64 {
    5
}
fn default_idle_timeout_seconds() -> u64 {
    600
}
fn default_max_lifetime_seconds() -> u64 {
    1800
}
fn default_statement_timeout_ms() -> u64 {
    10_000
}
fn default_lock_timeout_ms() -> u64 {
    5_000
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
    #[serde(default = "default_mfa_policy")]
    pub mfa_policy: String,
    #[serde(default)]
    pub mfa_encryption_key_file: String,
    #[serde(default)]
    pub mfa_encryption_key_base32: String,
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

    pub fn mfa_policy(&self) -> MfaPolicy {
        self.mfa_policy.parse().unwrap_or(MfaPolicy::Required)
    }

    /// Resolve the 32-byte MFA encryption key. `Ok(None)` only when policy is Off.
    /// File path wins over inline base32; errors if policy != Off and no key resolves
    /// or the decoded key is not 32 bytes.
    pub fn mfa_encryption_key(&self) -> anyhow::Result<Option<[u8; 32]>> {
        if self.mfa_policy() == MfaPolicy::Off {
            return Ok(None);
        }
        let key_str = if !self.mfa_encryption_key_file.is_empty() {
            std::fs::read_to_string(&self.mfa_encryption_key_file).with_context(|| {
                format!("reading MFA key file '{}'", self.mfa_encryption_key_file)
            })?
        } else if !self.mfa_encryption_key_base32.is_empty() {
            self.mfa_encryption_key_base32.clone()
        } else {
            anyhow::bail!(
                "mfa_policy != off but no MFA encryption key (set APP__AUTH__MFA_ENCRYPTION_KEY_FILE or _BASE32)"
            );
        };
        let bytes = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, key_str.trim())
            .ok_or_else(|| anyhow::anyhow!("MFA encryption key is not valid base32"))?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("MFA encryption key must decode to exactly 32 bytes"))?;
        Ok(Some(arr))
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
            mfa_policy: "required".into(),
            mfa_encryption_key_file: String::new(),
            mfa_encryption_key_base32: String::new(),
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

    #[test]
    fn server_and_db_settings_have_production_defaults() {
        // Build from an in-memory source (no global env) so this test is
        // deterministic and safe to run in parallel with `loads_settings_from_env`.
        let s: Settings = config::Config::builder()
            .set_override("server.port", 8080)
            .unwrap()
            .set_override("server.environment", "test")
            .unwrap()
            .set_override("database.url", "postgres://localhost/x")
            .unwrap()
            .set_override("database.max_connections", 5)
            .unwrap()
            .set_override("database.auto_migrate", false)
            .unwrap()
            .set_override("auth.jwt_public_key_pem", "PEM")
            .unwrap()
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();

        assert_eq!(s.server.request_timeout_seconds, 30);
        assert_eq!(s.server.max_body_bytes, 1_048_576);
        assert_eq!(s.server.auth_rate_limit_per_minute, 10);
        assert_eq!(s.server.auth_rate_limit_burst, 5);
        assert_eq!(s.database.min_connections, 1);
        assert_eq!(s.database.acquire_timeout_seconds, 5);
        assert_eq!(s.database.idle_timeout_seconds, 600);
        assert_eq!(s.database.max_lifetime_seconds, 1800);
        assert_eq!(s.database.statement_timeout_ms, 10_000);
        assert_eq!(s.database.lock_timeout_ms, 5_000);
    }

    #[test]
    fn mfa_policy_parses_and_defaults_required() {
        assert!(matches!(
            "off".parse::<MfaPolicy>().unwrap(),
            MfaPolicy::Off
        ));
        assert!(matches!(
            "optional".parse::<MfaPolicy>().unwrap(),
            MfaPolicy::Optional
        ));
        assert!(matches!(
            "required".parse::<MfaPolicy>().unwrap(),
            MfaPolicy::Required
        ));
        assert!(matches!(MfaPolicy::default(), MfaPolicy::Required));
        assert!("bogus".parse::<MfaPolicy>().is_err());
    }
}
