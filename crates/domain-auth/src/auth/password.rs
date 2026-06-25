/// Hash a plaintext password with bcrypt (cost factor 12).
pub fn hash_password(plaintext: &str) -> anyhow::Result<String> {
    Ok(bcrypt::hash(plaintext, 12)?)
}

/// Verify a plaintext password against a stored bcrypt hash.
/// Returns false on any error (malformed hash, mismatch).
pub fn verify_password(stored_hash: &str, plaintext: &str) -> bool {
    bcrypt::verify(plaintext, stored_hash).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(verify_password(&hash, "hunter2"));
        assert!(!verify_password(&hash, "wrong"));
    }
}
