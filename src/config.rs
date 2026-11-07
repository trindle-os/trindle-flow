//! The settings file, kept as a JSON object so unknown and older settings survive a save.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use base64::Engine;
use serde_json::{json, Map, Value};

pub const NO_FOLDER: &str = "Flow cannot tell where to keep its settings: set HOME or XDG_CONFIG_HOME to a full path.";

pub fn find_dir() -> Option<PathBuf> {
    match std::env::var_os("FLOW_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => trindle_common::files::config_home().map(|c| c.join("flow")),
    }
}

pub fn dir() -> PathBuf {
    find_dir().expect(NO_FOLDER)
}

pub fn path(name: &str) -> PathBuf {
    dir().join(name)
}

pub fn config_path() -> PathBuf {
    path("config.json")
}

pub fn ensure_dir() -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir())
}

pub mod key {
    pub const ROLE: &str = "role";
    pub const OTHER_SIDE: &str = "other_side";
    pub const PORT: &str = "port";
    pub const SERVER_HOST: &str = "server_host";
    pub const ADDRESS: &str = "address";
    pub const SERVER_FINGERPRINT: &str = "server_fingerprint";
    pub const SERVER_ID: &str = "server_id";
    pub const SERVER_NAME: &str = "server_name";
    pub const ASKING: &str = "asking";
    pub const PAIR_CODE: &str = "pair_code";
    pub const PAIR_MISSES: &str = "pair_misses";
    pub const PAIR_NUMBERS: &str = "pair_numbers";
    pub const ALLOWED_ID: &str = "allowed_id";
    pub const ALLOWED_CODE: &str = "allowed_code";
    pub const ALLOWED_UNTIL: &str = "allowed_until";
    pub const TOKEN: &str = "token";
    pub const SCREEN_WIDTH: &str = "screen_width";
    pub const SCREEN_HEIGHT: &str = "screen_height";
    pub const CONNECTED: &str = "connected";
    pub const SHORTCUT_ONLY: &str = "shortcut_only";
    pub const LEFT_COMPUTER: &str = "left_computer";
    pub const RIGHT_COMPUTER: &str = "right_computer";
    pub const TOP_COMPUTER: &str = "top_computer";
    pub const BOTTOM_COMPUTER: &str = "bottom_computer";
    pub const COMPUTERS: &str = "computers";
    pub const TOKENS: &str = "tokens";
    pub const DROP_ID: &str = "drop_id";
    pub const DROPPED: &str = "dropped";
    pub const POINTER_SPEED: &str = "pointer_speed";
    pub const DISCOVERY: &str = "discovery";
    pub const SHARE_CLIPBOARD: &str = "share_clipboard";
    pub const SHARE_FILES: &str = "share_files";
    pub const FILES_KEEP: &str = "files_keep";
    pub const FILES_FOLDER: &str = "files_folder";
}

pub fn defaults() -> Map<String, Value> {
    let v = json!({
        (key::ROLE): "controlled",
        (key::OTHER_SIDE): "right",
        (key::PORT): 24810,
        (key::SERVER_HOST): "",
        (key::ADDRESS): "",
        (key::SERVER_FINGERPRINT): "",
        (key::SERVER_ID): "",
        (key::SERVER_NAME): "",
        (key::ASKING): "",
        (key::PAIR_CODE): "",
        (key::PAIR_MISSES): 0,
        (key::PAIR_NUMBERS): 0,
        (key::ALLOWED_ID): "",
        (key::ALLOWED_CODE): "",
        (key::ALLOWED_UNTIL): 0,
        (key::TOKEN): "",
        (key::SCREEN_WIDTH): 1920,
        (key::SCREEN_HEIGHT): 1080,
        (key::CONNECTED): true,
        (key::SHORTCUT_ONLY): false,
        (key::LEFT_COMPUTER): "",
        (key::RIGHT_COMPUTER): "",
        (key::TOP_COMPUTER): "",
        (key::BOTTOM_COMPUTER): "",
        (key::COMPUTERS): {},
        (key::TOKENS): {},
        (key::DROP_ID): "",
        (key::DROPPED): [],
        (key::POINTER_SPEED): 1.0,
        (key::DISCOVERY): true,
        (key::SHARE_CLIPBOARD): true,
        (key::SHARE_FILES): true,
        (key::FILES_KEEP): true,
        (key::FILES_FOLDER): "",
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config(pub Map<String, Value>);

impl Config {
    pub fn str(&self, key: &str) -> String {
        match self.0.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }

    pub fn f64(&self, key: &str) -> f64 {
        let from = |v: Option<&Value>| match v {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
            _ => None,
        };
        from(self.0.get(key)).or_else(|| from(defaults().get(key))).unwrap_or(0.0)
    }

    pub fn i64(&self, key: &str) -> i64 {
        self.f64(key) as i64
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(Value::String(s)) => !s.is_empty(),
            _ => defaults().get(key).and_then(Value::as_bool).unwrap_or(false),
        }
    }

    pub fn controls(&self) -> bool {
        self.str(key::ROLE) == "control"
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_string(), value);
    }
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 24];
    crate::tls::random(&mut bytes).expect("the system gave Flow no randomness for its key");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn load() -> Config {
    let mut cfg = defaults();
    if let Ok(text) = fs::read_to_string(config_path()) {
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(saved)) => cfg.extend(saved),
            _ => {
                let bad = path("config.json.bad");
                if fs::rename(config_path(), &bad).is_ok() {
                    let said = format!("Flow's settings were damaged, so it starts afresh and the old ones are kept in {}. Pair the computers again.", bad.display());
                    eprintln!("{said}");
                    crate::page::tell("Flow", &said);
                }
            }
        }
    }
    let mut cfg = Config(cfg);
    if cfg.str(key::TOKEN).is_empty() {
        cfg.set(key::TOKEN, json!(random_token()));
        save_or_tell(&cfg);
    }
    cfg
}

pub fn write(name: &str, data: &[u8]) -> std::io::Result<()> {
    ensure_dir()?;
    trindle_common::files::write_private(&path(name), data)
}

pub struct EditLock(#[allow(dead_code)] fs::File);

pub fn edit_lock() -> Option<EditLock> {
    ensure_dir().ok()?;
    let f = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(path("settings.lock")).ok()?;
    // SAFETY: flock on a descriptor this function owns and keeps open.
    let got = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) } == 0;
    got.then_some(EditLock(f))
}

pub fn claim_running() -> Result<fs::File, String> {
    ensure_dir().map_err(|e| format!("could not make the settings folder: {e}"))?;
    let f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path("running.lock"))
        .map_err(|e| format!("could not start: {e}"))?;
    // SAFETY: as above; the lock goes when the program ends.
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Flow is already running for you, so this second copy has stopped.".into());
    }
    Ok(f)
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(&Value::Object(cfg.0.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    write("config.json", text.as_bytes())
}

static SAVE_FAILED: AtomicBool = AtomicBool::new(false);

pub fn save_or_tell(cfg: &Config) -> bool {
    match save(cfg) {
        Ok(()) => {
            SAVE_FAILED.store(false, Ordering::Relaxed);
            true
        }
        Err(e) => {
            if !SAVE_FAILED.swap(true, Ordering::Relaxed) {
                let said = format!("Flow could not save its settings, so a change may be lost: {e}");
                eprintln!("{said}");
                crate::page::tell("Flow", &said);
            }
            false
        }
    }
}

pub fn set_value(key: &str, value: Value) -> Config {
    let _held = edit_lock();
    let mut cfg = load();
    cfg.set(key, value);
    save_or_tell(&cfg);
    cfg
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn mtime() -> Option<SystemTime> {
    fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Mutex;

    pub static DIR_LOCK: Mutex<()> = Mutex::new(());

    pub struct TestHome {
        pub dir: PathBuf,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    impl TestHome {
        pub fn new() -> TestHome {
            let turn = DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut b = [0u8; 8];
            crate::tls::random(&mut b).unwrap();
            let dir = std::env::temp_dir().join(format!("flow-test-{:x}", u64::from_le_bytes(b)));
            fs::create_dir_all(&dir).unwrap();
            // SAFETY: tests that change the environment hold DIR_LOCK, so no
            // other thread in this test binary reads it at the same moment.
            unsafe {
                std::env::set_var("FLOW_CONFIG_DIR", &dir);
                std::env::set_var("HOME", &dir);
                std::env::remove_var("FLOW_PAGE_SETTINGS");
            }
            TestHome { dir, _turn: turn }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            // SAFETY: DIR_LOCK is still held; it goes after this.
            unsafe { std::env::remove_var("FLOW_PAGE_SETTINGS") };
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_test_home_leaves_nothing_behind() {
        let dir = {
            let home = TestHome::new();
            // SAFETY: the TestHome holds DIR_LOCK.
            unsafe { std::env::set_var("FLOW_PAGE_SETTINGS", home.dir.join("settings.ini")) };
            assert_eq!(trindle_common::files::home(), Some(home.dir.clone()));
            assert!(path("x").starts_with(&home.dir));
            home.dir.clone()
        };
        assert!(!dir.exists());
        assert!(std::env::var_os("FLOW_PAGE_SETTINGS").is_none());
    }

    #[test]
    fn keys_are_random_and_damaged_settings_are_set_aside() {
        let _home = TestHome::new();
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        fs::write(config_path(), "{\"token\": \"kept\", \"server_id\": \"desk1\"").unwrap();
        let cfg = load();
        assert_ne!(cfg.str(key::TOKEN), "kept");
        assert!(fs::read_to_string(path("config.json.bad")).unwrap().contains("desk1"));
        assert_ne!(cfg.str(key::TOKEN), "");
    }

    #[test]
    fn a_failed_save_is_said_once_until_one_works() {
        use std::os::unix::fs::PermissionsExt;
        let home = TestHome::new();
        let cfg = load();
        fs::set_permissions(&home.dir, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(!save_or_tell(&cfg));
        assert!(SAVE_FAILED.load(Ordering::Relaxed));
        assert!(!save_or_tell(&cfg));
        fs::set_permissions(&home.dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(save_or_tell(&cfg));
        assert!(!SAVE_FAILED.load(Ordering::Relaxed));
    }

    #[test]
    fn saves_at_the_same_moment_all_land() {
        let home = TestHome::new();
        let d = home.dir.clone();
        let writers: Vec<_> = (0..8)
            .map(|i| std::thread::spawn(move || (0..50).all(|_| write("same.json", format!("{i}").as_bytes()).is_ok())))
            .collect();
        assert!(writers.into_iter().all(|w| w.join().unwrap()));
        let left: Vec<_> = fs::read_dir(&d).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".part")).collect();
        assert!(left.is_empty());
        let held = claim_running().unwrap();
        assert!(claim_running().is_err());
        drop(held);
        assert!(claim_running().is_ok());
    }

    #[test]
    fn load_keeps_unknown_settings_and_makes_a_token() {
        let home = TestHome::new();
        let d = home.dir.clone();
        fs::write(d.join("config.json"), r#"{"push_px": 90, "extra": 7}"#).unwrap();
        let cfg = load();
        assert_eq!(cfg.i64("push_px"), 90);
        assert_eq!(cfg.str(key::TOKEN).len(), 32);
        assert_eq!(cfg.i64("extra"), 7);
        let again = load();
        assert_eq!(again.str(key::TOKEN), cfg.str(key::TOKEN));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(d.join("config.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
