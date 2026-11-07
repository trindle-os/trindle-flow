//! A small file the running Flow writes and the settings page reads.

use std::fs;

use serde_json::{json, Value};

use crate::config;

pub mod key {
    pub const ROLE: &str = "role";
    pub const RUNNING: &str = "running";
    pub const CONNECTED: &str = "connected";
    pub const PEERS: &str = "peers";
    pub const PEER_NAME: &str = "peer_name";
    pub const AWAY: &str = "away";
    pub const SIDE: &str = "side";
    pub const CAN_CROSS: &str = "can_cross";
    pub const FOUND: &str = "found";
    pub const ASKING: &str = "asking";
    pub const SERVER: &str = "server";
    pub const SERVER_NAME: &str = "server_name";
    pub const IDENTITY_CHANGED: &str = "identity_changed";
    pub const CLIPBOARD: &str = "clipboard";
    pub const CONTROL: &str = "control";
    pub const OFF: &str = "off";
}

pub fn empty() -> Value {
    json!({(key::RUNNING): false, (key::CONNECTED): false, (key::FOUND): []})
}

pub fn write(state: &Value) {
    let text = state.to_string();
    if fs::read_to_string(config::path("status.json")).is_ok_and(|old| old == text) {
        return;
    }
    let _ = config::write("status.json", text.as_bytes());
    crate::page::ring_menu(state);
}

pub fn read() -> Value {
    let mut merged = empty();
    if let Ok(Value::Object(saved)) =
        fs::read_to_string(config::path("status.json")).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null))
    {
        if let Some(m) = merged.as_object_mut() {
            m.extend(saved);
        }
    }
    merged
}

pub fn clear() {
    let _ = fs::remove_file(config::path("status.json"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TestHome;

    #[test]
    fn status_is_written_and_cleared() {
        let _home = TestHome::new();
        assert_eq!(read()["running"], json!(false));
        write(&json!({"running": true}));
        assert_eq!(read()["running"], json!(true));
        assert_eq!(read()["connected"], json!(false));
        let path = config::path("status.json");
        fs::write(&path, "changed by hand").unwrap();
        write(&json!({"running": true}));
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"running":true}"#);
        fs::write(&path, r#"{"running":true}"#).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        write(&json!({"running": true}));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        clear();
        assert_eq!(read()["running"], json!(false));
    }
}
