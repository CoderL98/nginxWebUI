//! Sign-in helpers: password hashing (the same scheme the Java version used),
//! in-memory sessions, and a small text captcha drawn as an SVG so no image
//! library is needed.
//!
//! Memory note: sessions live in one small map and are dropped after a timeout,
//! so the server keeps a few kilobytes per signed-in person rather than a
//! server-side session store.

use md5::{Digest, Md5};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub struct Session {
    pub admin_id: String,
    pub admin_name: String,
    pub code: String,
    pub expires: u64,
}

#[derive(Default)]
pub struct Sessions {
    map: HashMap<String, Session>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static mut SEED: u64 = 0;

fn rand_u64() -> u64 {
    // xorshift64*; good enough for session ids and captchas, no dependency.
    unsafe {
        if SEED == 0 {
            SEED = now() ^ (std::process::id() as u64) << 32 ^ 0x9E37_79B9_7F4A_7C15;
        }
        SEED ^= SEED << 13;
        SEED ^= SEED >> 7;
        SEED ^= SEED << 17;
        SEED
    }
}

pub fn random_token(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let n = rand_u64() as usize % ALPHABET.len();
        out.push(ALPHABET[n] as char);
    }
    out
}

impl Sessions {
    pub fn new_session(&mut self) -> String {
        let token = random_token(32);
        self.map.insert(
            token.clone(),
            Session {
                admin_id: String::new(),
                admin_name: String::new(),
                code: String::new(),
                expires: now() + 60 * 60 * 8,
            },
        );
        self.sweep();
        token
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

/// A four digit code drawn as an SVG picture, with light noise.
pub fn captcha_svg(code: &str) -> String {
    let mut svg = String::new();
    svg.push_str(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40" viewBox="0 0 120 40">"##,
    );
    svg.push_str(r##"<rect width="120" height="40" fill="#f2f2f2"/>"##);
    for _ in 0..5 {
        let x1 = rand_u64() % 120;
        let y1 = rand_u64() % 40;
        let x2 = rand_u64() % 120;
        let y2 = rand_u64() % 40;
        svg.push_str(&format!(
            r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#c6c6c6" stroke-width="1"/>"##
        ));
    }
    let colors = ["#1d1d1d", "#2b6cb0", "#b7791f", "#2f855a"];
    for (index, ch) in code.chars().enumerate() {
        let x = 14 + index as u64 * 26;
        let y = 28;
        let rotate = (rand_u64() % 20) as i64 - 10;
        let color = colors[(rand_u64() as usize) % colors.len()];
        svg.push_str(&format!(
            r#"<text x="{x}" y="{y}" font-family="monospace" font-size="24" fill="{color}" transform="rotate({rotate} {x} {y})">{ch}</text>"#
        ));
    }
    svg.push_str("</svg>");
    svg
}

pub fn random_code() -> String {
    const DIGITS: &[u8] = b"0123456789";
    let mut out = String::with_capacity(4);
    for _ in 0..4 {
        out.push(DIGITS[(rand_u64() as usize) % DIGITS.len()] as char);
    }
    out
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
}
