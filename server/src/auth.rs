//! Sign-in helpers: password hashing, in-memory sessions, captcha, and
//! rate limits.
//!
//! New passwords use argon2id. The Java-era `md5(pass) + md5("nginxWebUI")`
//! hash is still accepted and upgraded on a successful sign-in.
//! Session ids, captcha text, and salts come from the OS CSPRNG.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use md5::{Digest, Md5};
use rand_core::OsRng;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

pub const CAPTCHA_LEN: usize = 6;
pub const MAX_ANON_SESSIONS: usize = 128;
pub const LOGIN_FAIL_LIMIT: u32 = 5;
pub const CAPTCHA_WINDOW_SECS: u64 = 300;
pub const CAPTCHA_MAX_PER_WINDOW: u32 = 30;

/// Characters that are hard to mix up when reading a captcha.
const CAPTCHA_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const TOKEN_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `md5(pass) + md5("nginxWebUI")`, exactly like `EncodePassUtils.encode`.
pub fn encode_pass(pass: &str) -> String {
    if pass.is_empty() {
        return String::new();
    }
    format!("{}{}", md5_hex(pass), md5_hex("nginxWebUI"))
}

pub fn is_legacy_pass_hash(stored: &str) -> bool {
    stored.len() == 64 && stored.bytes().all(|b| b.is_ascii_hexdigit())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

pub fn hash_password(pass: &str) -> Result<String, String> {
    if pass.is_empty() {
        return Err("passTooShort".into());
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pass.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|err| err.to_string())
}

pub fn verify_password(pass: &str, stored: &str) -> bool {
    if stored.is_empty() {
        return false;
    }
    if stored.starts_with("$argon2") {
        let parsed = match PasswordHash::new(stored) {
            Ok(parsed) => parsed,
            Err(_) => return false,
        };
        return Argon2::default()
            .verify_password(pass.as_bytes(), &parsed)
            .is_ok();
    }
    constant_eq(stored.as_bytes(), encode_pass(pass).as_bytes())
}

pub fn password_needs_upgrade(stored: &str) -> bool {
    !stored.starts_with("$argon2")
}

pub struct Session {
    pub admin_id: String,
    pub admin_name: String,
    pub code: String,
    pub expires: u64,
    created: u64,
    seq: u64,
}

#[derive(Default)]
pub struct Sessions {
    map: HashMap<String, Session>,
    next_seq: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fill_random(buf: &mut [u8]) {
    if getrandom::getrandom(buf).is_err() {
        // Extremely unlikely; fall back to a weaker but non-deterministic mix
        // so we never silently reuse the old xorshift seed.
        let t = now().to_le_bytes();
        for (i, b) in buf.iter_mut().enumerate() {
            *b ^= t[i % t.len()];
            *b ^= (i as u8).wrapping_mul(0x5d);
        }
    }
}

pub fn random_token(len: usize) -> String {
    let mut raw = vec![0u8; len];
    fill_random(&mut raw);
    let mut out = String::with_capacity(len);
    for byte in raw {
        out.push(TOKEN_ALPHABET[(byte as usize) % TOKEN_ALPHABET.len()] as char);
    }
    out
}

impl Sessions {
    pub fn new_session(&mut self) -> String {
        self.sweep();
        self.evict_oldest_anon_if_full();
        let token = random_token(32);
        let now = now();
        self.next_seq = self.next_seq.saturating_add(1);
        self.map.insert(
            token.clone(),
            Session {
                admin_id: String::new(),
                admin_name: String::new(),
                code: String::new(),
                expires: now + 60 * 60 * 8,
                created: now,
                seq: self.next_seq,
            },
        );
        token
    }

    fn evict_oldest_anon_if_full(&mut self) {
        let anon: Vec<(String, u64)> = self
            .map
            .iter()
            .filter(|(_, s)| s.admin_id.is_empty())
            .map(|(k, s)| (k.clone(), s.seq))
            .collect();
        if anon.len() < MAX_ANON_SESSIONS {
            return;
        }
        if let Some((oldest, _)) = anon.into_iter().min_by_key(|(_, seq)| *seq) {
            self.map.remove(&oldest);
        }
    }

    pub fn anon_count(&self) -> usize {
        self.map.values().filter(|s| s.admin_id.is_empty()).count()
    }

    pub fn get(&self, token: &str) -> Option<&Session> {
        self.map.get(token)
    }

    pub fn touch(&mut self, token: &str) -> bool {
        match self.map.get_mut(token) {
            Some(session) if session.expires > now() => {
                session.expires = now() + 60 * 60 * 8;
                true
            }
            _ => false,
        }
    }

    pub fn set_code(&mut self, token: &str, code: &str) {
        if let Some(session) = self.map.get_mut(token) {
            session.code = code.to_string();
        }
    }

    pub fn sign_in(&mut self, token: &str, admin_id: &str, admin_name: &str) {
        if let Some(session) = self.map.get_mut(token) {
            session.admin_id = admin_id.to_string();
            session.admin_name = admin_name.to_string();
            session.expires = now() + 60 * 60 * 8;
        }
    }

    pub fn sign_out(&mut self, token: &str) {
        self.map.remove(token);
    }

    pub fn revoke_others(&mut self, keep: &str, admin_id: &str) {
        self.map
            .retain(|token, session| token == keep || session.admin_id != admin_id);
    }

    pub fn is_signed_in(&self, token: &str) -> bool {
        self.map
            .get(token)
            .map(|s| !s.admin_id.is_empty() && s.expires > now())
            .unwrap_or(false)
    }

    fn sweep(&mut self) {
        let now = now();
        self.map.retain(|_, s| s.expires > now);
    }
}

/// A short code drawn as an SVG picture, with light noise.
pub fn captcha_svg(code: &str) -> String {
    let width = 24 + code.len() * 26;
    let mut svg = String::new();
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="40" viewBox="0 0 {width} 40">"##
    ));
    svg.push_str(r##"<rect width="100%" height="40" fill="#f2f2f2"/>"##);
    for _ in 0..5 {
        let mut buf = [0u8; 8];
        fill_random(&mut buf);
        let x1 = (buf[0] as u64) % width as u64;
        let y1 = (buf[1] as u64) % 40;
        let x2 = (buf[2] as u64) % width as u64;
        let y2 = (buf[3] as u64) % 40;
        svg.push_str(&format!(
            r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#c6c6c6" stroke-width="1"/>"##
        ));
    }
    let colors = ["#1d1d1d", "#2b6cb0", "#b7791f", "#2f855a"];
    for (index, ch) in code.chars().enumerate() {
        let mut buf = [0u8; 2];
        fill_random(&mut buf);
        let x = 14 + index as u64 * 26;
        let y = 28;
        let rotate = (buf[0] as i64 % 20) - 10;
        let color = colors[(buf[1] as usize) % colors.len()];
        svg.push_str(&format!(
            r#"<text x="{x}" y="{y}" font-family="monospace" font-size="24" fill="{color}" transform="rotate({rotate} {x} {y})">{ch}</text>"#
        ));
    }
    svg.push_str("</svg>");
    svg
}

pub fn random_code() -> String {
    let mut raw = [0u8; CAPTCHA_LEN];
    fill_random(&mut raw);
    let mut out = String::with_capacity(CAPTCHA_LEN);
    for byte in raw {
        out.push(CAPTCHA_ALPHABET[(byte as usize) % CAPTCHA_ALPHABET.len()] as char);
    }
    out
}

pub fn codes_match(expected: &str, provided: &str) -> bool {
    constant_eq(
        expected.to_ascii_uppercase().as_bytes(),
        provided.trim().to_ascii_uppercase().as_bytes(),
    )
}

pub fn tokens_match(expected: &str, provided: &str) -> bool {
    constant_eq(expected.as_bytes(), provided.trim().as_bytes())
}

/// First-admin creation requires a current init token (none means already initialized).
pub fn first_admin_allowed(stored_token: Option<&str>, provided: &str) -> Result<(), &'static str> {
    match stored_token {
        None => Err("adminExists"),
        Some(expected) if !expected.is_empty() && tokens_match(expected, provided) => Ok(()),
        Some(_) => Err("initTokenError"),
    }
}

#[derive(Clone)]
struct FailState {
    count: u32,
    locked_until: u64,
}

#[derive(Clone)]
struct CaptchaWindow {
    start: u64,
    count: u32,
}

#[derive(Default)]
pub struct RateLimiter {
    login: HashMap<String, FailState>,
    captcha: HashMap<String, CaptchaWindow>,
}

impl RateLimiter {
    fn login_key(ip: &str, name: &str) -> String {
        format!("{ip}\n{}", name.trim().to_ascii_lowercase())
    }

    pub fn allow_login(&self, ip: &str, name: &str) -> Result<(), &'static str> {
        let key = Self::login_key(ip, name);
        if let Some(state) = self.login.get(&key) {
            if now() < state.locked_until {
                return Err("tooManyAttempts");
            }
        }
        Ok(())
    }

    pub fn fail_login(&mut self, ip: &str, name: &str) {
        let key = Self::login_key(ip, name);
        let now = now();
        let state = self.login.entry(key).or_insert(FailState {
            count: 0,
            locked_until: 0,
        });
        if now >= state.locked_until && state.count >= LOGIN_FAIL_LIMIT && state.locked_until > 0 {
            state.count = 0;
        }
        state.count = state.count.saturating_add(1);
        if state.count >= LOGIN_FAIL_LIMIT {
            let extra = (state.count - LOGIN_FAIL_LIMIT).min(4);
            let lock = 60u64.saturating_mul(1u64 << extra);
            state.locked_until = now.saturating_add(lock);
        }
    }

    pub fn succeed_login(&mut self, ip: &str, name: &str) {
        self.login.remove(&Self::login_key(ip, name));
    }

    pub fn allow_captcha(&mut self, ip: &str) -> Result<(), &'static str> {
        let now = now();
        let entry = self.captcha.entry(ip.to_string()).or_insert(CaptchaWindow {
            start: now,
            count: 0,
        });
        if now.saturating_sub(entry.start) >= CAPTCHA_WINDOW_SECS {
            entry.start = now;
            entry.count = 0;
        }
        if entry.count >= CAPTCHA_MAX_PER_WINDOW {
            return Err("tooManyAttempts");
        }
        entry.count = entry.count.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_does_not_revive_expired_session() {
        let mut sessions = Sessions::default();
        let token = sessions.new_session();
        sessions.sign_in(&token, "1", "admin");
        if let Some(session) = sessions.map.get_mut(&token) {
            session.expires = 1;
        }
        assert!(!sessions.is_signed_in(&token));
        assert!(!sessions.touch(&token));
        assert!(!sessions.is_signed_in(&token));
    }

    #[test]
    fn legacy_md5_still_verifies_and_is_marked_for_upgrade() {
        let stored = encode_pass("secret123");
        assert!(is_legacy_pass_hash(&stored));
        assert!(verify_password("secret123", &stored));
        assert!(!verify_password("wrong", &stored));
        assert!(password_needs_upgrade(&stored));
    }

    #[test]
    fn argon2_hash_verifies_and_does_not_need_upgrade() {
        let stored = hash_password("secret123").unwrap();
        assert!(stored.starts_with("$argon2"));
        assert!(verify_password("secret123", &stored));
        assert!(!verify_password("wrong", &stored));
        assert!(!password_needs_upgrade(&stored));
    }

    #[test]
    fn empty_stored_password_never_matches() {
        assert!(!verify_password("", ""));
        assert!(!verify_password("x", ""));
    }

    #[test]
    fn tokens_and_codes_use_csprng_and_expected_shape() {
        let a = random_token(32);
        let b = random_token(32);
        assert_eq!(a.len(), 32);
        assert_eq!(b.len(), 32);
        assert_ne!(a, b);
        let code = random_code();
        assert_eq!(code.len(), CAPTCHA_LEN);
        assert!(code.bytes().all(|c| CAPTCHA_ALPHABET.contains(&c)));
    }

    #[test]
    fn codes_match_ignores_case() {
        assert!(codes_match("AB23CD", "ab23cd"));
        assert!(!codes_match("AB23CD", "AB23CE"));
    }

    #[test]
    fn init_token_compare_is_exact() {
        assert!(tokens_match("abc", "abc"));
        assert!(tokens_match("abc", " abc "));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", ""));
    }

    #[test]
    fn first_admin_requires_live_init_token() {
        assert_eq!(
            first_admin_allowed(None, "anything").unwrap_err(),
            "adminExists"
        );
        assert_eq!(
            first_admin_allowed(Some("secret-token"), "wrong").unwrap_err(),
            "initTokenError"
        );
        first_admin_allowed(Some("secret-token"), "secret-token").unwrap();
        first_admin_allowed(Some("secret-token"), " secret-token ").unwrap();
    }

    #[test]
    fn rate_limiter_locks_after_repeated_failures() {
        let mut limiter = RateLimiter::default();
        limiter.allow_login("1.2.3.4", "admin").unwrap();
        for _ in 0..LOGIN_FAIL_LIMIT {
            limiter.fail_login("1.2.3.4", "admin");
        }
        assert_eq!(
            limiter.allow_login("1.2.3.4", "admin").unwrap_err(),
            "tooManyAttempts"
        );
        limiter.succeed_login("1.2.3.4", "admin");
        limiter.allow_login("1.2.3.4", "admin").unwrap();
    }

    #[test]
    fn captcha_rate_limiter_caps_requests() {
        let mut limiter = RateLimiter::default();
        for _ in 0..CAPTCHA_MAX_PER_WINDOW {
            limiter.allow_captcha("10.0.0.1").unwrap();
        }
        assert_eq!(
            limiter.allow_captcha("10.0.0.1").unwrap_err(),
            "tooManyAttempts"
        );
        limiter.allow_captcha("10.0.0.2").unwrap();
    }

    #[test]
    fn evicts_oldest_anonymous_session_when_full() {
        let mut sessions = Sessions::default();
        let mut tokens = Vec::new();
        for _ in 0..MAX_ANON_SESSIONS {
            tokens.push(sessions.new_session());
        }
        assert_eq!(sessions.anon_count(), MAX_ANON_SESSIONS);
        let extra = sessions.new_session();
        assert_eq!(sessions.anon_count(), MAX_ANON_SESSIONS);
        assert!(sessions.get(&extra).is_some());
        assert!(sessions.get(&tokens[0]).is_none());
    }

    #[test]
    fn revoke_others_keeps_current_session() {
        let mut sessions = Sessions::default();
        let keep = sessions.new_session();
        sessions.sign_in(&keep, "1", "admin");
        let other = sessions.new_session();
        sessions.sign_in(&other, "1", "admin");
        sessions.revoke_others(&keep, "1");
        assert!(sessions.is_signed_in(&keep));
        assert!(!sessions.is_signed_in(&other));
    }
}
