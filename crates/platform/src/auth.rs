use crate::server::AppError;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub exp: usize,
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
        let validation = Validation::new(Algorithm::RS256);
        Ok(JwtVerifier { key, validation })
    }

    pub fn verify(&self, token: &str) -> Result<AccessClaims, AppError> {
        decode::<AccessClaims>(token, &self.key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))
    }
}

pub fn require_scope(claims: &AccessClaims, scope: &str) -> Result<(), AppError> {
    if claims.has_scope(scope) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!("missing required scope: {scope}")))
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
