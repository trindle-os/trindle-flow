//! Unlocking the other computer's lock screen with the person's PIN.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::config;

#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables, non_upper_case_globals, non_snake_case,
        unused_imports, missing_docs, clippy::all)]
mod generated {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/unlock.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/unlock.xml");
}

use generated::trindle_unlock_v1::{self as wire, TrindleUnlockV1};

pub const NOT_FOUR_DIGITS: &str = "A PIN is 4 digits.";
const WAIT: Duration = Duration::from_secs(10);
const SOCKET: &str = "unlock.sock";

pub struct Pin(String);

impl Pin {
    pub fn new(text: String) -> Result<Pin, &'static str> {
        let pin = Pin(text);
        if pin.0.len() == 4 && pin.0.bytes().all(|b| b.is_ascii_digit()) {
            Ok(pin)
        } else {
            Err(NOT_FOUR_DIGITS)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        wipe_string(&mut self.0);
    }
}

fn wipe_string(text: &mut str) {
    // SAFETY: noughts are valid UTF-8, so the string stays a string.
    for b in unsafe { text.as_bytes_mut() } {
        // SAFETY: b is a valid, aligned byte of the string's own buffer.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

pub fn wipe_bytes(bytes: &mut [u8]) {
    for b in bytes {
        // SAFETY: b is a valid, aligned byte of the slice.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub done: bool,
    pub words: String,
}

impl Answer {
    pub fn done() -> Answer {
        Answer { done: true, words: "The screen is unlocked.".into() }
    }

    pub fn no(words: impl Into<String>) -> Answer {
        Answer { done: false, words: words.into() }
    }

    pub fn to_json(&self) -> Value {
        json!({"done": self.done, "words": self.words})
    }

    pub fn from_json(v: &Value) -> Answer {
        let words: String = v["words"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(120).collect();
        match v["done"].as_bool() {
            Some(true) => Answer::done(),
            _ if words.is_empty() => Answer::no("The other computer did not say why."),
            _ => Answer::no(words),
        }
    }
}

pub fn with_pin(pin: &Pin) -> Answer {
    match try_with_pin(pin) {
        Ok(a) => a,
        Err(words) => Answer::no(words),
    }
}

fn try_with_pin(pin: &Pin) -> Result<Answer, String> {
    let conn = Connection::connect_to_env().map_err(|_| "Flow cannot reach the desktop.".to_string())?;
    let (globals, mut queue) = registry_queue_init::<Heard>(&conn).map_err(|e| format!("the desktop did not answer ({e})"))?;
    let qh = queue.handle();
    let unlock: TrindleUnlockV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "This computer's Trindle does not let Flow unlock it. Update Trindle, and install Flow from Apps.".to_string())?;
    let mut heard = Heard(None);
    unlock.with_pin(pin.as_str().to_string());
    let began = Instant::now();
    while heard.0.is_none() && began.elapsed() < WAIT {
        queue.roundtrip(&mut heard).map_err(|e| format!("the desktop did not answer ({e})"))?;
        if heard.0.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    unlock.destroy();
    let _ = conn.flush();
    heard.0.ok_or_else(|| "The desktop did not answer in time.".to_string())
}

struct Heard(Option<Answer>);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Heard {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<TrindleUnlockV1, ()> for Heard {
    fn event(state: &mut Self, _: &TrindleUnlockV1, event: wire::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        state.0 = Some(match event {
            wire::Event::Done => Answer::done(),
            wire::Event::Refused { reason } => Answer::no(reason),
        });
    }
}

pub fn ask(id: &str) -> Answer {
    let mut line = String::new();
    let read = std::io::stdin().lock().take(64).read_line(&mut line);
    let typed = line.trim_end_matches(['\r', '\n']).to_string();
    wipe_string(&mut line);
    if read.is_err() {
        return Answer::no(NOT_FOUR_DIGITS);
    }
    let pin = match Pin::new(typed) {
        Ok(p) => p,
        Err(words) => return Answer::no(words),
    };
    hand_over(id, &pin).unwrap_or_else(Answer::no)
}

fn hand_over(id: &str, pin: &Pin) -> Result<Answer, String> {
    let mut stream = UnixStream::connect(config::path(SOCKET))
        .map_err(|_| "Flow is not running here, or this computer does not control the other one.".to_string())?;
    stream.set_read_timeout(Some(WAIT + Duration::from_secs(5))).map_err(|e| e.to_string())?;
    // Written by hand, so the only copy of the PIN is this buffer, which is wiped.
    let id = json!(id).to_string();
    let mut ask = Vec::with_capacity(id.len() + 32);
    for part in [b"{\"id\":".as_slice(), id.as_bytes(), b",\"pin\":\"", pin.as_str().as_bytes(), b"\"}\n"] {
        ask.extend_from_slice(part);
    }
    let sent = stream.write_all(&ask);
    wipe_bytes(&mut ask);
    sent.map_err(|_| "Flow stopped before it could ask.".to_string())?;
    let mut reply = String::new();
    BufReader::new(stream.take(4096)).read_line(&mut reply).map_err(|_| "The other computer did not answer in time.".to_string())?;
    let v: Value = serde_json::from_str(&reply).map_err(|_| "The other computer did not answer in time.".to_string())?;
    Ok(Answer::from_json(&v))
}

pub fn listen(send: impl Fn(&str, Pin) -> Answer + Send + 'static, log: crate::files::Log) -> Option<crate::Stopping> {
    let path = config::path(SOCKET);
    let _ = std::fs::remove_file(&path);
    // SAFETY: umask only sets this process's file-creation mask.
    let old = unsafe { libc::umask(0o177) };
    let bound = UnixListener::bind(&path);
    // SAFETY: as above, putting the old mask back.
    unsafe { libc::umask(old) };
    let listener = match bound.and_then(|l| l.set_nonblocking(true).map(|_| l)) {
        Ok(l) => l,
        Err(e) => {
            log(format!("unlock: cannot take PINs ({e})"));
            return None;
        }
    };
    Some(crate::Stopping::spawn(move |bell| {
        while bell.wait_for(std::os::fd::AsFd::as_fd(&listener)) {
            let (stream, _) = match listener.accept() {
                Ok(got) => got,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    bell.sleep(Duration::from_millis(200));
                    continue;
                }
            };
            if stream.set_nonblocking(false).is_err() {
                continue;
            }
            let answer = serve(&stream, &send);
            log(format!("unlock: {}", if answer.done { "the other computer is unlocked".to_string() } else { format!("not unlocked: {}", answer.words) }));
            let mut out = &stream;
            let _ = writeln!(out, "{}", answer.to_json());
        }
    }))
}

fn serve(stream: &UnixStream, send: &impl Fn(&str, Pin) -> Answer) -> Answer {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let read = BufReader::new(stream.take(256)).read_line(&mut line);
    let mut v: Value = if read.is_ok() { serde_json::from_str(&line).unwrap_or(Value::Null) } else { Value::Null };
    wipe_string(&mut line);
    let id = v["id"].as_str().unwrap_or("").to_string();
    let typed = match v.get_mut("pin") {
        Some(Value::String(s)) => std::mem::take(s),
        _ => String::new(),
    };
    match Pin::new(typed) {
        Ok(pin) => send(&id, pin),
        Err(words) => Answer::no(words),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_four_digits_and_nothing_else() {
        assert!(Pin::new("0429".into()).is_ok());
        for bad in ["", "123", "12345", "12a4", " 123", "１２３４"] {
            assert_eq!(Pin::new(bad.into()).err(), Some(NOT_FOUR_DIGITS), "{bad:?}");
        }
    }

    #[test]
    fn a_wiped_pin_leaves_only_noughts() {
        let mut text = String::from("0429");
        wipe_string(&mut text);
        assert_eq!(text.as_bytes(), &[0, 0, 0, 0]);
    }

    #[test]
    fn answers_cross_the_link_and_keep_their_words() {
        let refused = Answer::no("Wrong PIN. 4 tries left.");
        assert_eq!(Answer::from_json(&refused.to_json()), refused);
        assert_eq!(Answer::from_json(&Answer::done().to_json()), Answer::done());
        assert!(!Answer::from_json(&Value::Null).done);
        let odd = Answer::from_json(&json!({"done": false, "words": "a\nb"}));
        assert_eq!(odd.words, "ab");
    }

    #[test]
    fn the_running_part_hands_the_pin_on_and_says_the_answer() {
        let _home = config::tests::TestHome::new();
        let listening = listen(
            |id, pin| if id == "laptop" && pin.as_str() == "0429" { Answer::done() } else { Answer::no("Wrong PIN. 4 tries left.") },
            std::sync::Arc::new(|_| {}),
        );
        assert_eq!(hand_over("laptop", &Pin::new("0429".into()).unwrap()), Ok(Answer::done()));
        assert_eq!(hand_over("laptop", &Pin::new("1111".into()).unwrap()).unwrap().words, "Wrong PIN. 4 tries left.");
        drop(listening);
        assert!(hand_over("laptop", &Pin::new("0429".into()).unwrap()).is_err());
        let _again = listen(|_, _| Answer::done(), std::sync::Arc::new(|_| {}));
        assert_eq!(hand_over("laptop", &Pin::new("0429".into()).unwrap()), Ok(Answer::done()));
    }
}
