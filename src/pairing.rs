//! Pairing two computers with SPAKE2, tied to the TLS connection they share.

use base64::Engine;
use ring::hmac;
use serde_json::json;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::config::key;
use crate::{config, page};

pub const TRIES: i64 = 3;
pub const NUMBERS: i64 = 3;

pub const ALLOW_SECONDS: u64 = 120;

pub fn new_code() -> Result<String, String> {
    loop {
        let mut b = [0u8; 2];
        crate::tls::random(&mut b)?;
        let n = u16::from_be_bytes(b);
        if n < 60000 {
            return Ok(format!("{:04}", n % 10000));
        }
    }
}

pub fn is_code(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn unb64(v: &serde_json::Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap_or("")).unwrap_or_default()
}

pub struct Check {
    half: Spake2<Ed25519Group>,
    tie: Vec<u8>,
}

pub fn start(code: &str, asking: (&str, &str), controls_fp: &str, tie: &[u8], controls: bool) -> (Check, Vec<u8>) {
    let a = Identity::new(format!("flow asking {} {}", asking.0, asking.1.to_lowercase()).as_bytes());
    let b = Identity::new(format!("flow controls {}", controls_fp.to_lowercase()).as_bytes());
    let pw = Password::new(code.as_bytes());
    let (half, msg) = if controls { Spake2::start_b(&pw, &a, &b) } else { Spake2::start_a(&pw, &a, &b) };
    (Check { half, tie: tie.to_vec() }, msg)
}

pub struct Agreed {
    key: hmac::Key,
    tie: Vec<u8>,
}

impl Check {
    pub fn finish(self, theirs: &[u8]) -> Option<Agreed> {
        let key = self.half.finish(theirs).ok()?;
        Some(Agreed { key: hmac::Key::new(hmac::HMAC_SHA256, &key), tie: self.tie })
    }
}

impl Agreed {
    fn said(&self, by_controls: bool) -> Vec<u8> {
        [if by_controls { &b"controls"[..] } else { &b"asking"[..] }, &self.tie].concat()
    }

    pub fn proof(&self, by_controls: bool) -> Vec<u8> {
        hmac::sign(&self.key, &self.said(by_controls)).as_ref().to_vec()
    }

    pub fn fits(&self, by_controls: bool, proof: &[u8]) -> bool {
        hmac::verify(&self.key, &self.said(by_controls), proof).is_ok()
    }
}

pub fn allow_into(cfg: &mut config::Config, id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("flow allow ID".into());
    }
    cfg.set(key::ALLOWED_ID, json!(id));
    cfg.set(key::ALLOWED_UNTIL, json!(config::now_secs() + ALLOW_SECONDS));
    Ok(())
}

pub fn number_into(cfg: &mut config::Config, code: &str) -> Result<(), String> {
    if !is_code(code) {
        return Err("The number is the four digits the other computer shows.".into());
    }
    cfg.set(key::ALLOWED_CODE, json!(code));
    cfg.set(key::ALLOWED_UNTIL, json!(config::now_secs() + ALLOW_SECONDS));
    Ok(())
}

pub fn allow(id: &str, code: &str) -> Result<(), String> {
    let _held = config::edit_lock();
    let mut cfg = config::load();
    allow_into(&mut cfg, id)?;
    number_into(&mut cfg, code)?;
    config::save(&cfg).map_err(|e| format!("could not save the settings: {e}"))
}

pub fn take_allowance(id: &str) -> Option<String> {
    let _held = config::edit_lock();
    let mut cfg = config::load();
    let code = cfg.str(key::ALLOWED_CODE);
    let ok = !id.is_empty()
        && cfg.str(key::ALLOWED_ID) == id
        && is_code(&code)
        && cfg.i64(key::ALLOWED_UNTIL) as u64 >= config::now_secs();
    if !ok {
        return None;
    }
    cfg.set(key::ALLOWED_ID, json!(""));
    cfg.set(key::ALLOWED_CODE, json!(""));
    cfg.set(key::ALLOWED_UNTIL, json!(0));
    page::set_field(&mut cfg, page::NUMBER, "");
    config::save_or_tell(&cfg).then_some(code)
}

pub fn use_into(cfg: &mut config::Config, id: &str) -> Result<String, String> {
    let found = crate::status::read()[crate::status::key::FOUND].as_array().cloned().unwrap_or_default();
    let Some(entry) = found.iter().find(|f| f["id"].as_str() == Some(id)) else {
        return Err("That computer is not among those Flow has found. Check Flow is running on it and set to control.".into());
    };
    let name = page::plain(entry["name"].as_str().unwrap_or("the other computer"), 40);
    let code = new_code()?;
    cfg.set(key::SERVER_ID, json!(id));
    cfg.set(key::SERVER_NAME, json!(name));
    cfg.set(key::SERVER_HOST, entry["host"].clone());
    cfg.set(key::SERVER_FINGERPRINT, json!(""));
    cfg.set(key::ASKING, json!(id));
    cfg.set(key::PAIR_CODE, json!(code));
    cfg.set(key::PAIR_MISSES, json!(0));
    cfg.set(key::PAIR_NUMBERS, json!(1));
    cfg.set(key::CONNECTED, json!(true));
    Ok(format!("Asking {name}. On {name}, open Flow's settings, pick this computer and type {code}."))
}

pub fn use_found(id: &str) -> Result<String, String> {
    let _held = config::edit_lock();
    let mut cfg = config::load();
    let said = use_into(&mut cfg, id)?;
    config::save(&cfg).map_err(|e| format!("could not save the settings: {e}"))?;
    Ok(said)
}

pub fn missed_into(cfg: &mut config::Config) -> String {
    let misses = cfg.i64(key::PAIR_MISSES) + 1;
    if misses < TRIES {
        cfg.set(key::PAIR_MISSES, json!(misses));
        return "The number typed on the other computer did not match. Type it again there.".into();
    }
    let numbers = cfg.i64(key::PAIR_NUMBERS) + 1;
    let fresh = if numbers <= NUMBERS { new_code().ok() } else { None };
    let Some(code) = fresh else {
        unpair_into(cfg);
        page::set_field(cfg, page::USE, "");
        return "Pairing stopped after too many wrong numbers. Pick the computer again to start over.".into();
    };
    cfg.set(key::PAIR_CODE, json!(code));
    cfg.set(key::PAIR_MISSES, json!(0));
    cfg.set(key::PAIR_NUMBERS, json!(numbers));
    format!("Too many wrong numbers, so the number is now {code}. Type that on the other computer.")
}

pub fn missed() -> (config::Config, String) {
    let _held = config::edit_lock();
    let mut cfg = config::load();
    let said = missed_into(&mut cfg);
    config::save_or_tell(&cfg);
    (cfg, said)
}

pub fn unpair_into(cfg: &mut config::Config) {
    cfg.set(key::TOKEN, json!(config::random_token()));
    cfg.set(key::TOKENS, json!({}));
    for name in [key::SERVER_HOST, key::SERVER_FINGERPRINT, key::SERVER_ID, key::SERVER_NAME, key::ASKING, key::PAIR_CODE] {
        cfg.set(name, json!(""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_that_cannot_be_cleared_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = crate::config::tests::TestHome::new();
        config::set_value(key::ALLOWED_ID, json!("laptop1"));
        config::set_value(key::ALLOWED_CODE, json!("0042"));
        config::set_value(key::ALLOWED_UNTIL, json!(config::now_secs() + 60));
        std::fs::set_permissions(&home.dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(take_allowance("laptop1"), None);
        std::fs::set_permissions(&home.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(take_allowance("laptop1").as_deref(), Some("0042"));
        assert_eq!(take_allowance("laptop1"), None);
    }

    fn run(asking_code: &str, controls_code: &str, ties: (&[u8], &[u8]), controls_fp: (&str, &str)) -> bool {
        let (a, to_b) = start(asking_code, ("laptop1", "AA"), controls_fp.0, ties.0, false);
        let (b, to_a) = start(controls_code, ("laptop1", "aa"), controls_fp.1, ties.1, true);
        let (Some(a), Some(b)) = (a.finish(&to_a), b.finish(&to_b)) else { return false };
        a.fits(true, &b.proof(true)) && b.fits(false, &a.proof(false))
    }

    #[test]
    fn the_check_passes_only_with_the_same_number_on_one_connection() {
        let tie = [7u8; 32];
        assert!(run("1234", "1234", (&tie, &tie), ("FF", "ff")));
        assert!(!run("1234", "1235", (&tie, &tie), ("ff", "ff")));
        assert!(!run("1234", "1234", (&tie, &[8u8; 32]), ("ff", "ff")));
        assert!(!run("1234", "1234", (&tie, &tie), ("ee", "ff")));
        let (a, to_b) = start("1234", ("x", "aa"), "ff", &tie, false);
        let (b, to_a) = start("1234", ("x", "aa"), "ff", &tie, true);
        let (a, b) = (a.finish(&to_a).unwrap(), b.finish(&to_b).unwrap());
        assert!(!a.fits(true, &a.proof(false)));
        assert!(b.fits(false, &a.proof(false)));
    }

    #[test]
    fn only_four_digits_make_a_number() {
        assert!(!is_code("123") && !is_code("12345") && !is_code("12a4") && !is_code("١٢٣٤"));
    }

    #[test]
    fn allowing_needs_the_pick_and_the_number_and_works_once() {
        let _home = crate::config::tests::TestHome::new();
        assert_eq!(take_allowance("laptop1"), None);
        assert!(allow("laptop1", "12").is_err());
        let _held = config::edit_lock();
        let mut cfg = config::load();
        allow_into(&mut cfg, "laptop1").unwrap();
        config::save(&cfg).unwrap();
        drop(_held);
        assert_eq!(take_allowance("laptop1"), None);
        allow("laptop1", "0042").unwrap();
        assert_eq!(take_allowance("someone-else"), None);
        assert_eq!(take_allowance("laptop1").as_deref(), Some("0042"));
        assert_eq!(take_allowance("laptop1"), None);
    }

    #[test]
    fn wrong_tries_change_the_number_then_stop_asking() {
        let _home = crate::config::tests::TestHome::new();
        let mut cfg = config::load();
        cfg.set(key::ASKING, json!("desk1"));
        cfg.set(key::SERVER_ID, json!("desk1"));
        cfg.set(key::PAIR_CODE, json!("1111"));
        cfg.set(key::PAIR_NUMBERS, json!(1));
        let mut codes = vec![cfg.str(key::PAIR_CODE)];
        for round in 1..=NUMBERS {
            for _ in 1..TRIES {
                missed_into(&mut cfg);
                assert_eq!(cfg.str(key::PAIR_CODE), *codes.last().unwrap());
            }
            missed_into(&mut cfg);
            if round < NUMBERS {
                codes.push(cfg.str(key::PAIR_CODE));
                assert!(is_code(codes.last().unwrap()));
                assert_eq!(cfg.str(key::ASKING), "desk1");
            }
        }
        assert_eq!(cfg.str(key::ASKING), "");
        assert_eq!(cfg.str(key::PAIR_CODE), "");
        assert_eq!(cfg.str(key::SERVER_ID), "");
    }
}
