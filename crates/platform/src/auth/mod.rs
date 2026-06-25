mod revocation;
pub use revocation::{NoopRevocationChecker, RevocationChecker};

use crate::server::AppError;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub exp: usize,
    pub iat: usize,
    pub jti: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(rename = "type", default)]
    pub token_type: String,
}

impl AccessClaims {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Clone)]
pub struct JwtVerifier {
    key: DecodingKey,
    validation: Validation,
}

impl JwtVerifier {
    /// Build a verifier from an RSA public key in PEM form (RS256).
    pub fn from_rsa_pem(pem: &str) -> anyhow::Result<JwtVerifier> {
        let key = DecodingKey::from_rsa_pem(pem.as_bytes())?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_aud = false;
        Ok(JwtVerifier { key, validation })
    }

    pub fn verify(&self, token: &str) -> Result<AccessClaims, AppError> {
        self.decode::<AccessClaims>(token)
    }

    /// Decode + validate (signature, exp) any claims shape signed with this key.
    pub fn decode<T: serde::de::DeserializeOwned>(&self, token: &str) -> Result<T, AppError> {
        decode::<T>(token, &self.key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))
    }
}

pub fn require_scope(claims: &AccessClaims, scope: &str) -> Result<(), AppError> {
    if claims.has_scope(scope) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "missing required scope: {scope}"
        )))
    }
}

use axum::extract::{FromRef, FromRequestParts};
use std::sync::Arc;

/// Extractor that verifies a Bearer token and yields its claims.
/// Works with any axum state from which `Arc<JwtVerifier>` can be borrowed.
pub struct Authenticated(pub AccessClaims);

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for Authenticated
where
    Arc<JwtVerifier>: FromRef<S>,
    Arc<dyn RevocationChecker>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing Authorization header".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("expected Bearer token".into()))?;
        let verifier = Arc::<JwtVerifier>::from_ref(state);
        let claims = verifier.verify(token)?;

        let checker = Arc::<dyn RevocationChecker>::from_ref(state);
        if checker
            .is_revoked(&claims)
            .await
            .map_err(AppError::Internal)?
        {
            return Err(AppError::Unauthorized("token revoked".into()));
        }
        Ok(Authenticated(claims))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(scopes: &[&str]) -> AccessClaims {
        AccessClaims {
            sub: "user-1".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
            iat: 0,
            jti: "test-jti".into(),
            email: None,
            token_type: "user".into(),
        }
    }

    #[test]
    fn has_scope_checks_membership() {
        let c = claims(&["admin", "read:accounts:own"]);
        assert!(c.has_scope("admin"));
        assert!(!c.has_scope("write:accounts"));
    }

    #[test]
    fn require_scope_rejects_missing() {
        let c = claims(&["read:accounts:own"]);
        assert!(require_scope(&c, "admin").is_err());
        assert!(require_scope(&c, "read:accounts:own").is_ok());
    }
}
