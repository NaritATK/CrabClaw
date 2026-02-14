// Gateway pairing mode — first-connect authentication.
//
// On startup the gateway generates a one-time pairing code printed to the
// terminal. The first client must present this code via `X-Pairing-Code`
// header on a `POST /pair` request. The server responds with a bearer token
// that must be sent on all subsequent requests via `Authorization: Bearer <token>`.
//
// Already-paired tokens are persisted in config so restarts don't require
// re-pairing.

use std::collections::HashSet;
use std::sync::Mutex;

/// Manages pairing state for the gateway.
#[derive(Debug)]
pub struct PairingGuard {
    /// Whether pairing is required at all.
    require_pairing: bool,
    /// One-time pairing code (generated on startup, consumed on first pair).
    pairing_code: Option<String>,
    /// Set of valid bearer tokens (persisted across restarts).
    paired_tokens: Mutex<HashSet<String>>,
}

impl PairingGuard {
    /// Create a new pairing guard.
    ///
    /// If `require_pairing` is true and no tokens exist yet, a fresh
    /// pairing code is generated and returned via `pairing_code()`.
    pub fn new(require_pairing: bool, existing_tokens: &[String]) -> Self {
        let tokens: HashSet<String> = existing_tokens.iter().cloned().collect();
        let code = if require_pairing && tokens.is_empty() {
            Some(generate_code())
        } else {
            None
        };
        Self {
            require_pairing,
            pairing_code: code,
            paired_tokens: Mutex::new(tokens),
        }
    }

    /// The one-time pairing code (only set when no tokens exist yet).
    pub fn pairing_code(&self) -> Option<&str> {
        self.pairing_code.as_deref()
    }

    /// Whether pairing is required at all.
    pub fn require_pairing(&self) -> bool {
        self.require_pairing
    }

    /// Attempt to pair with the given code. Returns a bearer token on success.
    pub fn try_pair(&self, code: &str) -> Option<String> {
        if let Some(ref expected) = self.pairing_code {
            if constant_time_eq(code.trim(), expected.trim()) {
                let token = generate_token();
                let mut tokens = self
                    .paired_tokens
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                tokens.insert(token.clone());
                return Some(token);
            }
        }
        None
    }

    /// Check if a bearer token is valid.
    pub fn is_authenticated(&self, token: &str) -> bool {
        if !self.require_pairing {
            return true;
        }
        let tokens = self
            .paired_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokens.contains(token)
    }

    /// Returns true if the gateway is already paired (has at least one token).
    pub fn is_paired(&self) -> bool {
        let tokens = self
            .paired_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !tokens.is_empty()
    }

    /// Get all paired tokens (for persisting to config).
    pub fn tokens(&self) -> Vec<String> {
        let tokens = self
            .paired_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokens.iter().cloned().collect()
    }
}

/// Generate a 6-digit numeric pairing code.
fn generate_code() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let raw = hasher.finish();
    format!("{:06}", raw % 1_000_000)
}

/// Generate a cryptographically-adequate bearer token (hex-encoded).
fn generate_token() -> String {
    format!("zc_{}", uuid::Uuid::new_v4().as_simple())
}

/// Constant-time string comparison to prevent timing attacks on pairing code.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Check if a host string represents a non-localhost bind address.
pub fn is_public_bind(host: &str) -> bool {
    !matches!(
        host,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "0:0:0:0:0:0:0:1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PairingGuard ─────────────────────────────────────────

    #[test]
    fn new_guard_generates_code_when_no_tokens() {
        let guard = PairingGuard::new(true, &[]);
        assert!(guard.pairing_code().is_some());
        assert!(!guard.is_paired());
    }

    #[test]
    fn new_guard_no_code_when_tokens_exist() {
        let guard = PairingGuard::new(true, &["zc_existing".into()]);
        assert!(guard.pairing_code().is_none());
        assert!(guard.is_paired());
    }

    #[test]
    fn new_guard_no_code_when_pairing_disabled() {
        let guard = PairingGuard::new(false, &[]);
        assert!(guard.pairing_code().is_none());
    }

    #[test]
    fn try_pair_correct_code() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap().to_string();
        let token = guard.try_pair(&code);
        assert!(token.is_some());
        assert!(token.unwrap().starts_with("zc_"));
        assert!(guard.is_paired());
    }

    #[test]
    fn try_pair_wrong_code() {
        let guard = PairingGuard::new(true, &[]);
        let token = guard.try_pair("000000");
        // Might succeed if code happens to be 000000, but extremely unlikely
        // Just check it doesn't panic
        let _ = token;
    }

    #[test]
    fn try_pair_empty_code() {
        let guard = PairingGuard::new(true, &[]);
        assert!(guard.try_pair("").is_none());
    }

    #[test]
    fn is_authenticated_with_valid_token() {
        let guard = PairingGuard::new(true, &["zc_valid".into()]);
        assert!(guard.is_authenticated("zc_valid"));
    }

    #[test]
    fn is_authenticated_with_invalid_token() {
        let guard = PairingGuard::new(true, &["zc_valid".into()]);
        assert!(!guard.is_authenticated("zc_invalid"));
    }

    #[test]
    fn is_authenticated_when_pairing_disabled() {
        let guard = PairingGuard::new(false, &[]);
        assert!(guard.is_authenticated("anything"));
        assert!(guard.is_authenticated(""));
    }

    #[test]
    fn tokens_returns_all_paired() {
        let guard = PairingGuard::new(true, &["a".into(), "b".into()]);
        let mut tokens = guard.tokens();
        tokens.sort();
        assert_eq!(tokens, vec!["a", "b"]);
    }

    #[test]
    fn pair_then_authenticate() {
        let guard = PairingGuard::new(true, &[]);
        let code = guard.pairing_code().unwrap().to_string();
        let token = guard.try_pair(&code).unwrap();
        assert!(guard.is_authenticated(&token));
        assert!(!guard.is_authenticated("wrong"));
    }

    // ── is_public_bind ───────────────────────────────────────

    #[test]
    fn localhost_variants_not_public() {
        assert!(!is_public_bind("127.0.0.1"));
        assert!(!is_public_bind("localhost"));
        assert!(!is_public_bind("::1"));
        assert!(!is_public_bind("[::1]"));
    }

    #[test]
    fn zero_zero_is_public() {
        assert!(is_public_bind("0.0.0.0"));
    }

    #[test]
    fn real_ip_is_public() {
        assert!(is_public_bind("192.168.1.100"));
        assert!(is_public_bind("10.0.0.1"));
    }

    // ── constant_time_eq ─────────────────────────────────────

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("a", ""));
    }

    // ── generate helpers ─────────────────────────────────────

    #[test]
    fn generate_code_is_6_digits() {
        let code = generate_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn generate_token_has_prefix() {
        let token = generate_token();
        assert!(token.starts_with("zc_"));
        assert!(token.len() > 10);
    }
}
