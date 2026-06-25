use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use platform::auth::AccessClaims;
use serde::{Deserialize, Serialize};

/// Claims for a refresh JWT (separate shape from the access token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshClaims {
    pub sub: String,
    pub iat: usize,
    pub exp: usize,
    pub jti: String,
    #[serde(rename = "type")]
    pub token_type: String,
}

/// Signs access + refresh tokens with an RSA private key (RS256).
#[derive(Clone)]
pub struct JwtIssuer {
    key: EncodingKey,
    access_ttl_seconds: i64,
    refresh_ttl_days: i64,
}

impl JwtIssuer {
    pub fn from_rsa_pem(
        pem: &str,
        access_ttl_seconds: i64,
        refresh_ttl_days: i64,
    ) -> anyhow::Result<JwtIssuer> {
        let key = EncodingKey::from_rsa_pem(pem.as_bytes())?;
        Ok(JwtIssuer {
            key,
            access_ttl_seconds,
            refresh_ttl_days,
        })
    }

    pub fn access_ttl_seconds(&self) -> i64 {
        self.access_ttl_seconds
    }

    /// Issue a signed access token. Returns the compact token and its claims.
    pub fn issue_access(
        &self,
        user_id: i64,
        email: &str,
        scopes: Vec<String>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(String, AccessClaims)> {
        let exp = (now + Duration::seconds(self.access_ttl_seconds)).timestamp() as usize;
        let claims = AccessClaims {
            sub: format!("user-{user_id}"),
            scopes,
            exp,
            iat: now.timestamp() as usize,
            jti: uuid::Uuid::new_v4().to_string(),
            email: Some(email.to_string()),
            token_type: "user".to_string(),
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
        Ok((token, claims))
    }

    /// Issue a signed refresh token. Returns (jti, token, expires_at).
    pub fn issue_refresh(
        &self,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(String, String, DateTime<Utc>)> {
        let jti = uuid::Uuid::new_v4().to_string();
        let expires_at = now + Duration::days(self.refresh_ttl_days);
        let claims = RefreshClaims {
            sub: format!("user-{user_id}"),
            iat: now.timestamp() as usize,
            exp: expires_at.timestamp() as usize,
            jti: jti.clone(),
            token_type: "refresh".to_string(),
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
        Ok((jti, token, expires_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::auth::JwtVerifier;

    const TEST_PRIV_PEM: &str = include_str!("../../tests/fixtures/test_priv.pem");
    const TEST_PUB_PEM: &str = include_str!("../../tests/fixtures/test_pub.pem");

    #[test]
    fn issued_access_token_verifies_with_public_key() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let now = chrono::Utc::now();
        let (token, claims) = issuer
            .issue_access(42, "a@b.c", vec!["admin".into()], now)
            .unwrap();
        assert_eq!(claims.sub, "user-42");
        assert_eq!(claims.token_type, "user");
        assert!(!claims.jti.is_empty());

        let verifier = JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap();
        let verified = verifier.verify(&token).unwrap();
        assert_eq!(verified.sub, "user-42");
        assert_eq!(verified.email.as_deref(), Some("a@b.c"));
        assert!(verified.has_scope("admin"));
    }

    #[test]
    fn issued_refresh_token_has_jti_and_expiry() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let now = chrono::Utc::now();
        let (jti, token, expires_at) = issuer.issue_refresh(42, now).unwrap();
        assert!(!jti.is_empty());
        assert!(!token.is_empty());
        assert!(expires_at > now);
    }
}
