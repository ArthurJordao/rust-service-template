use chrono::{DateTime, Utc};
use totp_rs::{Algorithm, Secret, TOTP};

/// A pluggable second-factor verifier. v1 impl is TOTP; WebAuthn/email-OTP can add
/// impls later keyed by the `auth_mfa_factor.type` column.
pub trait FactorVerifier: Send + Sync {
    fn generate_secret(&self) -> String;
    fn provisioning_uri(&self, secret_base32: &str, account: &str) -> anyhow::Result<String>;
    fn verify(&self, secret_base32: &str, code: &str, now: DateTime<Utc>) -> bool;
}

pub struct TotpVerifier {
    issuer: String,
}

impl TotpVerifier {
    pub fn new(issuer: String) -> TotpVerifier {
        TotpVerifier { issuer }
    }

    fn totp(&self, secret_base32: &str, account: &str) -> anyhow::Result<TOTP> {
        let bytes = Secret::Encoded(secret_base32.to_string())
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("bad totp secret: {e:?}"))?;
        TOTP::new(
            Algorithm::SHA1,
            6,
            1, // skew: accept ±1 step for clock drift
            30,
            bytes,
            Some(self.issuer.clone()),
            account.to_string(),
        )
        .map_err(|e| anyhow::anyhow!("totp init: {e}"))
    }

    #[cfg(test)]
    pub fn current_code(&self, secret_base32: &str, now: DateTime<Utc>) -> anyhow::Result<String> {
        let totp = self.totp(secret_base32, "test")?;
        Ok(totp.generate(now.timestamp() as u64))
    }
}

impl FactorVerifier for TotpVerifier {
    fn generate_secret(&self) -> String {
        Secret::generate_secret().to_encoded().to_string()
    }

    fn provisioning_uri(&self, secret_base32: &str, account: &str) -> anyhow::Result<String> {
        Ok(self.totp(secret_base32, account)?.get_url())
    }

    fn verify(&self, secret_base32: &str, code: &str, now: DateTime<Utc>) -> bool {
        match self.totp(secret_base32, "verify") {
            Ok(totp) => totp.check(code, now.timestamp() as u64),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn generated_secret_verifies_current_code() {
        let v = TotpVerifier::new("rust-service".into());
        let secret = v.generate_secret();
        let now = Utc::now();
        // derive the current code via a fresh TOTP over the same secret
        let code = v.current_code(&secret, now).unwrap();
        assert!(v.verify(&secret, &code, now));
        assert!(!v.verify(&secret, "000000", now));
    }

    #[test]
    fn provisioning_uri_is_otpauth() {
        let v = TotpVerifier::new("rust-service".into());
        let uri = v.provisioning_uri(&v.generate_secret(), "a@b.c").unwrap();
        assert!(uri.starts_with("otpauth://totp/"));
    }
}
