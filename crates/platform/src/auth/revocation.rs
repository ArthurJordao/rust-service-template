use crate::auth::AccessClaims;

/// Decides whether an otherwise-valid access token must be rejected
/// (logged-out jti, or issued before the user's tokens_valid_from epoch).
#[async_trait::async_trait]
pub trait RevocationChecker: Send + Sync {
    async fn is_revoked(&self, claims: &AccessClaims) -> anyhow::Result<bool>;
}

/// Default checker for contexts with no revocation store (and tests). Never revokes.
pub struct NoopRevocationChecker;

#[async_trait::async_trait]
impl RevocationChecker for NoopRevocationChecker {
    async fn is_revoked(&self, _claims: &AccessClaims) -> anyhow::Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AccessClaims;

    fn claims() -> AccessClaims {
        AccessClaims {
            sub: "user-1".into(),
            scopes: vec![],
            exp: 9_999_999_999,
            iat: 0,
            jti: "j".into(),
            email: None,
            token_type: "user".into(),
            amr: vec![],
        }
    }

    #[tokio::test]
    async fn noop_never_revokes() {
        let c = NoopRevocationChecker;
        assert!(!c.is_revoked(&claims()).await.unwrap());
    }
}
