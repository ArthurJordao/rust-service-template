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

    pub fn refresh_ttl_seconds(&self) -> i64 {
        self.refresh_ttl_days * 86_400
    }

    /// Issue a signed access token. Returns the compact token and its claims.
    pub fn issue_access(
        &self,
        user_id: i64,
        email: &str,
        scopes: Vec<String>,
        amr: Vec<String>,
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
            amr,
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

    /// Short-lived (5 min) single-purpose token gating the next MFA step.
    pub fn issue_mfa_token(
        &self,
        user_id: i64,
        purpose: MfaPurpose,
        now: DateTime<Utc>,
    ) -> anyhow::Result<String> {
        let claims = MfaTokenClaims {
            sub: format!("user-{user_id}"),
            iat: now.timestamp() as usize,
            exp: (now + Duration::minutes(5)).timestamp() as usize,
            token_type: purpose.token_type().to_string(),
        };
        Ok(encode(&Header::new(Algorithm::RS256), &claims, &self.key)?)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MfaPurpose {
    Pending,
    Enroll,
}

impl MfaPurpose {
    fn token_type(self) -> &'static str {
        match self {
            MfaPurpose::Pending => "mfa_pending",
            MfaPurpose::Enroll => "mfa_enroll",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaTokenClaims {
    pub sub: String,
    pub iat: usize,
    pub exp: usize,
    #[serde(rename = "type")]
    pub token_type: String,
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
            .issue_access(42, "a@b.c", vec!["admin".into()], vec!["pwd".into()], now)
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

    #[test]
    fn access_token_carries_amr() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let (_t, claims) = issuer
            .issue_access(
                1,
                "a@b.c",
                vec![],
                vec!["pwd".into(), "totp".into()],
                chrono::Utc::now(),
            )
            .unwrap();
        assert_eq!(claims.amr, vec!["pwd".to_string(), "totp".to_string()]);
    }

    #[test]
    fn mfa_token_has_purpose_type() {
        let issuer = JwtIssuer::from_rsa_pem(TEST_PRIV_PEM, 900, 7).unwrap();
        let token = issuer
            .issue_mfa_token(1, MfaPurpose::Enroll, chrono::Utc::now())
            .unwrap();
        let verifier = JwtVerifier::from_rsa_pem(TEST_PUB_PEM).unwrap();
        let claims: MfaTokenClaims = verifier.decode(&token).unwrap();
        assert_eq!(claims.token_type, "mfa_enroll");
        assert_eq!(claims.sub, "user-1");
    }
}
