use crate::models::Account;
use platform::auth::AccessClaims;
use platform::server::AppError;

/// Owner-or-admin policy (mirrors the Haskell AccessPolicy for Account).
pub fn can_access(claims: &AccessClaims, account: &Account) -> bool {
    if claims.has_scope("admin") {
        return true;
    }
    claims.has_scope("read:accounts:own")
        && claims.sub == format!("user-{}", account.auth_user_id)
}

pub fn authorize(claims: &AccessClaims, account: &Account) -> Result<(), AppError> {
    if can_access(claims, account) {
        Ok(())
    } else {
        Err(AppError::Forbidden("not allowed to access this account".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::auth::AccessClaims;

    fn account(uid: i64) -> Account {
        Account {
            id: 1,
            email: "a@b.c".into(),
            name: "A".into(),
            auth_user_id: uid,
            created_at: chrono::Utc::now(),
            created_by_cid: "cid".into(),
        }
    }
    fn claims(sub: &str, scopes: &[&str]) -> AccessClaims {
        AccessClaims {
            sub: sub.into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            exp: 9_999_999_999,
        }
    }

    #[test]
    fn admin_can_access_any_account() {
        assert!(can_access(&claims("user-999", &["admin"]), &account(1)));
    }

    #[test]
    fn owner_with_scope_can_access_own_account() {
        assert!(can_access(&claims("user-7", &["read:accounts:own"]), &account(7)));
    }

    #[test]
    fn non_owner_without_admin_cannot_access() {
        assert!(!can_access(&claims("user-8", &["read:accounts:own"]), &account(7)));
    }

    #[test]
    fn owner_without_scope_cannot_access() {
        assert!(!can_access(&claims("user-7", &[]), &account(7)));
    }
}
