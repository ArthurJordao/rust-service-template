use crate::auth::password::verify_password;
use crate::models::User;
use platform::server::AppError;

/// A freshly issued access + refresh token pair plus metadata for persistence.
#[derive(Debug, Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub refresh_jti: String,
    pub refresh_expires_at: chrono::DateTime<chrono::Utc>,
    pub expires_in: i64,
}

/// A bcrypt hash of a throwaway value, used to spend ~the same time hashing on
/// the "user not found" path as on the real-user path (reduces timing signal).
const DUMMY_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKcEeO3.9I8H8sJ8q8sJ8q8sJ8q8sJ8q8sJ8";

/// Add the `admin` scope when the email is in the admin bootstrap list.
pub fn effective_scopes(
    email: &str,
    mut db_scopes: Vec<String>,
    admin_emails: &[String],
) -> Vec<String> {
    let is_admin_email = admin_emails.iter().any(|e| e == email);
    if is_admin_email && !db_scopes.iter().any(|s| s == "admin") {
        db_scopes.push("admin".to_string());
    }
    db_scopes
}

/// Verify credentials. Returns the user on success, `401` otherwise.
pub fn check_credentials<'a>(user: Option<&'a User>, password: &str) -> Result<&'a User, AppError> {
    match user {
        Some(u) if verify_password(&u.password_hash, password) => Ok(u),
        Some(_) => Err(AppError::Unauthorized("invalid credentials".into())),
        None => {
            // Spend comparable time so presence of the account isn't timing-detectable.
            let _ = verify_password(DUMMY_HASH, password);
            Err(AppError::Unauthorized("invalid credentials".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::password::hash_password;

    fn user(email: &str, password: &str) -> User {
        User {
            id: 1,
            email: email.into(),
            password_hash: hash_password(password).unwrap(),
            tokens_valid_from: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            created_by_cid: "cid".into(),
        }
    }

    #[test]
    fn effective_scopes_bootstraps_admin_for_admin_email() {
        let scopes = effective_scopes(
            "boss@x.y",
            vec!["read:accounts:own".into()],
            &["boss@x.y".to_string()],
        );
        assert!(scopes.contains(&"admin".to_string()));
        assert!(scopes.contains(&"read:accounts:own".to_string()));
    }

    #[test]
    fn effective_scopes_leaves_non_admin_email_untouched() {
        let scopes = effective_scopes("u@x.y", vec!["read:accounts:own".into()], &[]);
        assert_eq!(scopes, vec!["read:accounts:own".to_string()]);
    }

    #[test]
    fn check_credentials_accepts_correct_password() {
        let u = user("a@b.c", "pw");
        assert!(check_credentials(Some(&u), "pw").is_ok());
    }

    #[test]
    fn check_credentials_rejects_wrong_password() {
        let u = user("a@b.c", "pw");
        assert!(check_credentials(Some(&u), "nope").is_err());
    }

    #[test]
    fn check_credentials_rejects_missing_user() {
        assert!(check_credentials(None, "pw").is_err());
    }
}
