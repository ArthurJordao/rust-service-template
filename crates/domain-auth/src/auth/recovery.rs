use crate::auth::password::{hash_password, verify_password};
use totp_rs::Secret;

/// 10 single-use recovery codes formatted `xxxxx-xxxxx` (lowercased base32, no padding).
pub fn generate_recovery_codes() -> Vec<String> {
    (0..10)
        .map(|_| {
            let raw = Secret::generate_secret()
                .to_encoded()
                .to_string()
                .to_lowercase();
            let s: String = raw
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(10)
                .collect();
            format!("{}-{}", &s[0..5], &s[5..10])
        })
        .collect()
}

pub fn hash_recovery_code(code: &str) -> anyhow::Result<String> {
    hash_password(code)
}

pub fn verify_recovery_code(hash: &str, code: &str) -> bool {
    verify_password(hash, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_ten_unique_codes() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), 10);
        let uniq: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(uniq.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 11 && c.contains('-')));
    }

    #[test]
    fn hash_then_verify() {
        let code = "abcde-fghij".to_string();
        let hash = hash_recovery_code(&code).unwrap();
        assert!(verify_recovery_code(&hash, &code));
        assert!(!verify_recovery_code(&hash, "wrong-code0"));
    }
}
