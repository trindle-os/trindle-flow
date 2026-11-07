//! Flow as a Trindle plugin: the Settings page values and the ring text.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Map, Value};
use trindle_common::files::{home, write_private};
use trindle_common::ini::Ini;

use crate::config::key;
use crate::{config, pairing, panel, status};

const SETTINGS: &[&str] =
    &[key::OTHER_SIDE, key::LEFT_COMPUTER, key::RIGHT_COMPUTER, key::TOP_COMPUTER, key::BOTTOM_COMPUTER, key::POINTER_SPEED, key::SHORTCUT_ONLY, key::SHARE_CLIPBOARD, key::SHARE_FILES, key::FILES_KEEP, key::ADDRESS, key::DISCOVERY, key::FILES_FOLDER];
const CONTROLS: &str = "controls";
pub const USE: &str = "use_computer";
const ALLOW: &str = "allow_computer";
pub const NUMBER: &str = "allow_number";
const SEEN: &str = "page_seen";
const SECTION: &str = "Settings";

pub fn installed() -> bool {
    values_path().is_some()
}

fn values_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FLOW_PAGE_SETTINGS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let boxed = std::env::current_exe().is_ok_and(|e| e.starts_with("/app"));
    boxed.then(|| plugin_file("settings.ini")).flatten()
}

fn plugin_file(name: &str) -> Option<PathBuf> {
    home().map(|h| h.join(name))
}

fn read_values() -> Map<String, Value> {
    let Some(page) = values_path().and_then(|p| Ini::read(&p).ok()) else { return Map::new() };
    [CONTROLS, USE, ALLOW, NUMBER].iter().chain(SETTINGS).filter_map(|k| Some((k.to_string(), json!(page.get(SECTION, k)?)))).collect()
}

pub fn reflect() {
    if values_path().is_none() {
        return;
    }
    let _held = config::edit_lock();
    let mut cfg = config::load();
    let want = if cfg.controls() { "true" } else { "false" };
    if read_values().get(CONTROLS).and_then(Value::as_str).unwrap_or("false") == want {
        return;
    }
    if set_field(&mut cfg, CONTROLS, want) {
        config::save_or_tell(&cfg);
    }
}

pub fn set_field(cfg: &mut config::Config, key: &str, value: &str) -> bool {
    let Some(path) = values_path() else { return false };
    let Ok(mut page) = Ini::read(&path) else { return false };
    if !page.set(SECTION, key, value) || page.write(&path).is_err() {
        return false;
    }
    let mut seen = cfg.0.get(SEEN).and_then(Value::as_object).cloned().unwrap_or_default();
    seen.insert(key.into(), json!(value));
    cfg.set(SEEN, Value::Object(seen));
    true
}

pub fn take() -> Vec<String> {
    let page = read_values();
    if page.is_empty() {
        return Vec::new();
    }
    let _held = config::edit_lock();
    let mut cfg = config::load();
    let seen = cfg.0.get(SEEN).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut problems = Vec::new();
    let mut changed = false;
    for (key, value) in &page {
        if seen.get(key) == Some(value) {
            continue;
        }
        changed = true;
        let text = value.as_str().unwrap_or("");
        if key == CONTROLS {
            cfg.set(key::ROLE, json!(if text == "true" { "control" } else { "controlled" }));
        } else if key == USE {
            if text.is_empty() {
                if !cfg.str(key::SERVER_ID).is_empty() {
                    pairing::unpair_into(&mut cfg);
                }
            } else if cfg.str(key::SERVER_ID) != text {
                if let Err(e) = pairing::use_into(&mut cfg, text) {
                    problems.push(e);
                }
            }
        } else if key == ALLOW {
            if let Some((id, _)) = text.rsplit_once('-') {
                if let Err(e) = pairing::allow_into(&mut cfg, id) {
                    problems.push(e);
                }
            }
        } else if key == NUMBER {
            if pairing::is_code(text) {
                let _ = pairing::number_into(&mut cfg, text);
            }
        } else if SETTINGS.contains(&key.as_str()) {
            match panel::checked(key, text) {
                Ok(v) => cfg.set(key, v),
                Err(e) => problems.push(e),
            }
        }
    }
    if changed {
        cfg.set(SEEN, Value::Object(page));
        if let Err(e) = config::save(&cfg) {
            problems.push(format!("Flow could not save its settings: {e}"));
        }
    }
    problems
}

pub fn take_and_tell(log: impl Fn(String)) {
    for problem in take() {
        log(problem.clone());
        tell("Flow could not take a setting", &problem);
    }
}

pub fn tell(summary: &str, body: &str) {
    if values_path().is_none() {
        return;
    }
    if let Some(path) = plugin_file("notify.jsonl") {
        let _ = trindle_common::notify::notify_line(&path, summary, body);
    }
}

pub fn identity_changed(who: &str) -> String {
    format!(
        "{who} is not the computer it was: its identity has changed, usually because Flow was reinstalled there. \
         Flow will not use it until the two are paired again: choose Use {who} in Flow's ring menu and type the number on {who}."
    )
}

pub fn tell_status() {
    let body = status_line(&crate::status::read());
    tell("Flow", &body);
    println!("{body}");
}

fn status_line(state: &Value) -> String {
    let cfg = config::load();
    if !cfg.bool(key::CONNECTED) {
        return "Disconnected. Choose Connect to share the mouse and keyboard again.".to_string();
    }
    if cfg.controls() {
        return controlling_status(state, &cfg);
    }
    if !cfg.str(key::ASKING).is_empty() {
        let who = plain(&cfg.str(key::SERVER_NAME), 40);
        return format!("Asking {who}, number {}. On {who}, open Flow's settings, pick this computer and type the number.", cfg.str(key::PAIR_CODE));
    }
    let who = plain(&Some(cfg.str(key::SERVER_NAME)).filter(|n| !n.is_empty()).unwrap_or(cfg.str(key::SERVER_HOST)), 40);
    let running = config::claim_running().is_err();
    let mut body = if !running {
        "Flow is not running. Allow it to run from login in Settings, Permissions.".to_string()
    } else if state[status::key::CONNECTED].as_bool() == Some(true) {
        format!("Connected to {who}.")
    } else if state[status::key::IDENTITY_CHANGED].as_bool() == Some(true) {
        identity_changed(&who)
    } else if who.is_empty() {
        match state[status::key::FOUND].as_array().and_then(|f| f.first()).and_then(|f| f["name"].as_str()).map(|n| plain(n, 40)) {
            Some(name) => format!("Found {name}. Pick it in Flow's settings, under Computer that controls this one."),
            None => "Looking for a computer to control this one. Set Flow on the other computer to control."
                .to_string(),
        }
    } else {
        format!("Looking for {who}.")
    };
    if running && state[status::key::CONTROL].as_bool() == Some(false) {
        body.push_str(" Allow Flow to control your keyboard, mouse and clipboard in Settings, Permissions, so the other computer can use this one.");
    }
    body
}

pub fn write_computers(cfg: &config::Config) {
    if values_path().is_none() {
        return;
    }
    if cfg.0.get(key::COMPUTERS).and_then(Value::as_object).is_none_or(|c| c.len() < 2) {
        let _ = plugin_file("computers.txt").map(fs::remove_file);
        return;
    }
    let mut text = String::from("|Nobody\n");
    for (id, name) in cfg.0.get(key::COMPUTERS).and_then(Value::as_object).into_iter().flatten() {
        let name = plain(name.as_str().unwrap_or(id), 40);
        text.push_str(&format!("{id}|{name}\n"));
    }
    put("computers.txt", &text);
}

fn put(name: &str, text: &str) {
    let Some(path) = plugin_file(name) else { return };
    if fs::read_to_string(&path).ok().as_deref() != Some(text) {
        let _ = write_private(&path, text.as_bytes());
    }
}

pub fn plain(name: &str, most: usize) -> String {
    name.chars().filter(|c| !c.is_control() && !"<>&|".contains(*c)).take(most).collect()
}

fn lists(state: &Value, cfg: &config::Config) {
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let mut found: Vec<(String, String)> = Vec::new();
    let mut asking: Vec<(String, String)> = Vec::new();
    if cfg.controls() {
        for ask in state[status::key::ASKING].as_array().into_iter().flatten().take(8) {
            let value = format!("{}-{}", text(ask, "id"), ask["asked"].as_u64().unwrap_or(0));
            asking.push((value, plain(&text(ask, "name"), 40)));
        }
    } else {
        let paired = cfg.str(key::SERVER_ID);
        if !paired.is_empty() {
            found.push((paired.clone(), plain(&cfg.str(key::SERVER_NAME), 40)));
        }
        for f in state[status::key::FOUND].as_array().into_iter().flatten().take(8) {
            let id = text(f, "id");
            if !id.is_empty() && id != paired && id.len() <= 48 {
                found.push((id, plain(&text(f, "name"), 40)));
            }
        }
    }
    for (name, choices) in [("found.txt", found), ("asking.txt", asking)] {
        if choices.is_empty() {
            let _ = plugin_file(name).map(fs::remove_file);
            continue;
        }
        let mut out = String::from("|Nobody\n");
        for (value, words) in choices {
            out.push_str(&format!("{value}|{words}\n"));
        }
        put(name, &out);
    }
}

fn short(name: &str) -> String {
    let name = plain(name, 40);
    if name.chars().count() <= 18 {
        name
    } else {
        name.chars().take(17).collect::<String>() + "…"
    }
}

fn menu_items(state: &Value, cfg: &config::Config) -> Vec<Value> {
    let mut items = Vec::new();
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    if cfg.controls() {
        if state[status::key::AWAY].as_bool() != Some(true) {
            for peer in state[status::key::PEERS].as_array().into_iter().flatten().take(4) {
                items.push(json!({"label": format!("Go to {}", short(&text(peer, "name"))), "args": ["goto", text(peer, "id")]}));
            }
        }
        for peer in state[status::key::PEERS].as_array().into_iter().flatten().take(4) {
            let id = text(peer, "id");
            if !id.is_empty() && id.len() <= 64 {
                items.push(json!({"label": format!("Unlock {}", short(&text(peer, "name"))), "unlock": id}));
            }
        }
        for peer in state[status::key::PEERS].as_array().into_iter().flatten().take(4) {
            items.push(json!({"label": format!("Disconnect {}", short(&text(peer, "name"))), "args": ["drop", text(peer, "id")]}));
        }
        return items;
    }
    if state[status::key::CONNECTED].as_bool() != Some(true) && cfg.str(key::ASKING).is_empty() {
        for found in state[status::key::FOUND].as_array().into_iter().flatten().take(6) {
            let id = text(found, "id");
            if id.is_empty() || id.len() > 64 {
                continue;
            }
            items.push(json!({"label": format!("Use {}", short(&text(found, "name"))), "args": ["use", id]}));
        }
    }
    items
}

pub fn ring_menu(state: &Value) {
    if values_path().is_none() {
        return;
    }
    let line = status_line(state);
    let headline = match line.find(". ") {
        Some(i) => line[..=i].to_string(),
        None => line.clone(),
    };
    let cfg = config::load();
    let dropped = cfg.0.get(key::DROPPED).and_then(Value::as_array).is_some_and(|d| !d.is_empty());
    let mut hidden: Vec<&str> = match (cfg.bool(key::CONNECTED), dropped) {
        (false, _) => vec!["disconnect"],
        (true, false) => vec!["connect"],
        (true, true) => vec![],
    };
    if !cfg.controls() || state[status::key::AWAY].as_bool() != Some(true) {
        hidden.push("home");
    }
    let menu = json!({"headline": headline, "hidden": hidden, "items": menu_items(state, &cfg)}).to_string();
    put("ring.json", &menu);
    put("status.txt", &line);
    lists(state, &cfg);
}

fn controlling_status(state: &Value, cfg: &config::Config) -> String {
    let running = config::claim_running().is_err();
    if !running {
        return "Flow is not running. Allow it to run from login in Settings, Permissions.".to_string();
    }
    let side = crate::capture::Side::parse(&cfg.str(key::OTHER_SIDE)).unwrap_or(crate::capture::Side::Right);
    let asking = state[status::key::ASKING].as_array().and_then(|a| a.first());
    let mut body = if let Some(ask) = asking {
        format!(
            "{} asks to be controlled. Pick it in Flow's settings, under Computer asking to be controlled, and type the number it shows.",
            plain(ask["name"].as_str().unwrap_or("A computer"), 40)
        )
    } else if state[status::key::CONNECTED].as_bool() == Some(true) {
        let who = plain(state[status::key::PEER_NAME].as_str().unwrap_or("the other computer"), 40);
        format!("Connected to {who}. Push the pointer off the side of the screen {} to use it.", side.said_screen())
    } else {
        "Waiting for the other computer. In Flow's settings there, pick this computer.".to_string()
    };
    if state[status::key::CAN_CROSS].as_bool() == Some(false) {
        body.push_str(" Allow Flow to share your mouse and keyboard with another computer in Settings, Permissions, so the pointer can cross.");
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TestHome;

    #[test]
    fn a_changed_identity_offers_only_pairing_again() {
        let _home = TestHome::new();
        config::set_value(key::ROLE, json!("controlled"));
        config::set_value(key::SERVER_NAME, json!("Desk"));
        let _flow_is_running = config::claim_running().unwrap();
        let state = json!({"identity_changed": true, "found": [{"id": "desk1", "name": "Desk"}]});
        assert!(status_line(&state).starts_with("Desk is not the computer it was"));
        let items = menu_items(&state, &config::load());
        assert!(items.iter().any(|i| i["label"] == "Use Desk" && i["args"] == json!(["use", "desk1"])));
        assert!(items.iter().all(|i| !i["label"].as_str().unwrap_or("").contains("Trust")));
    }

    #[test]
    fn a_controlling_computer_offers_to_unlock_each_connected_one() {
        let home = TestHome::new();
        let d = home.dir.clone();
        let page = d.join("settings.ini");
        // SAFETY: DIR_LOCK is held, so no other test reads the environment now.
        unsafe { std::env::set_var("FLOW_PAGE_SETTINGS", &page) };
        config::set_value(key::ROLE, json!("control"));
        let cfg = config::load();
        assert!(cfg.controls());
        let state = json!({"peers": [{"id": "abc123", "name": "Office"}, {"id": "", "name": "Nameless"}]});
        let items = menu_items(&state, &cfg);
        let unlocks: Vec<&Value> = items.iter().filter(|i| i.get("unlock").is_some()).collect();
        assert_eq!(unlocks.len(), 1);
        assert_eq!(unlocks[0]["unlock"], "abc123");
        assert_eq!(unlocks[0]["label"], "Unlock Office");
        assert!(unlocks[0].get("args").is_none());
    }

    #[test]
    fn names_from_elsewhere_cannot_break_a_line_or_add_markup() {
        assert_eq!(plain("<i>Desk</i>|evil\nrow", 40), "iDesk/ievilrow");
        assert_eq!(short("<b>A very long computer name indeed</b>"), "bA very long comp…");
    }

    #[test]
    fn address_discovery_and_folder_come_from_the_page() {
        let home = TestHome::new();
        let page = home.dir.join("settings.ini");
        // SAFETY: DIR_LOCK is held, so no other test reads the environment now.
        unsafe { std::env::set_var("FLOW_PAGE_SETTINGS", &page) };
        fs::write(&page, "[Settings]\naddress=100.64.0.7\ndiscovery=false\nfiles_folder=/home/john/Flow\n").unwrap();
        assert!(take().is_empty());
        let cfg = config::load();
        assert_eq!(cfg.str(key::ADDRESS), "100.64.0.7");
        assert!(!cfg.bool(key::DISCOVERY));
        assert_eq!(cfg.str(key::FILES_FOLDER), "/home/john/Flow");

        fs::write(&page, "[Settings]\naddress=evil;rm\ndiscovery=false\nfiles_folder=not/a/full/path\n").unwrap();
        assert_eq!(take().len(), 2);
        let cfg = config::load();
        assert_eq!(cfg.str(key::ADDRESS), "100.64.0.7");
        assert_eq!(cfg.str(key::FILES_FOLDER), "/home/john/Flow");

        fs::write(&page, "[Settings]\naddress=\ndiscovery=true\nfiles_folder=\n").unwrap();
        assert!(take().is_empty());
        let cfg = config::load();
        assert_eq!(cfg.str(key::ADDRESS), "");
        assert!(cfg.bool(key::DISCOVERY));
        assert_eq!(cfg.str(key::FILES_FOLDER), "");
    }

    #[test]
    fn page_values_are_taken_once_and_checked() {
        let home = TestHome::new();
        let d = home.dir.clone();
        let page = d.join("settings.ini");
        // SAFETY: DIR_LOCK is held, so no other test reads the environment now.
        unsafe { std::env::set_var("FLOW_PAGE_SETTINGS", &page) };
        fs::write(&page, "[Settings]\nshare_files=false\npointer_speed=9\ncontrols=true\nallow_computer=abc123-77\n").unwrap();
        let problems = take();
        let cfg = config::load();
        assert!(!cfg.bool(key::SHARE_FILES));
        assert_eq!(cfg.f64(key::POINTER_SPEED), 1.0);
        assert_eq!(problems.len(), 1);
        assert!(cfg.controls());
        assert_eq!(cfg.str(key::ALLOWED_ID), "abc123");

        config::set_value(key::SHARE_FILES, json!(true));
        config::set_value(key::ROLE, json!("controlled"));
        assert!(take().is_empty());
        let cfg = config::load();
        assert!(cfg.bool(key::SHARE_FILES));
        assert!(!cfg.controls());

        config::set_value(key::SERVER_ID, json!("desk1"));
        fs::write(&page, "[Settings]\nshare_files=false\npointer_speed=Faster\ncontrols=true\nallow_computer=abc123-77\nuse_computer=\n").unwrap();
        assert!(take().is_empty());
        let cfg = config::load();
        assert_eq!(cfg.f64(key::POINTER_SPEED), 1.5);
        assert_eq!(cfg.str(key::SERVER_ID), "");
        fs::write(&page, "[Settings]\nshare_files=false\npointer_speed=Faster\ncontrols=true\nallow_computer=abc123-77\nuse_computer=nobodyhere\n").unwrap();
        assert_eq!(take().len(), 1);

        fs::write(&page, "[Settings]\ncontrols=true\nallow_computer=abc123-78\nallow_number=00\n").unwrap();
        take();
        assert_eq!(config::load().str(key::ALLOWED_CODE), "");
        fs::write(&page, "[Settings]\ncontrols=true\nallow_computer=abc123-78\nallow_number=0042\n").unwrap();
        take();
        assert_eq!(pairing::take_allowance("abc123").as_deref(), Some("0042"));
        assert!(fs::read_to_string(&page).unwrap().contains("allow_number=\n"));
        assert!(take().is_empty());
        assert_eq!(config::load().str(key::ALLOWED_CODE), "");
    }
}
