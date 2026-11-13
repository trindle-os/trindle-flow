//! What the Flow card on the Plugins page reads and changes; every value is checked here, once.

use serde_json::{json, Map, Value};

use crate::config::key;
use crate::{config, status};

enum Kind {
    Role,
    Side,
    Computer,
    Speed,
    Bool,
    Host,
    Folder,
}

pub const SPEEDS: &[(&str, f64)] = &[("Slower", 0.6), ("Normal", 1.0), ("Faster", 1.5), ("Much faster", 2.0)];

const EDITABLE: &[(&str, Kind)] = &[
    (key::ROLE, Kind::Role),
    (key::OTHER_SIDE, Kind::Side),
    (key::ADDRESS, Kind::Host),
    (key::LEFT_COMPUTER, Kind::Computer),
    (key::RIGHT_COMPUTER, Kind::Computer),
    (key::TOP_COMPUTER, Kind::Computer),
    (key::BOTTOM_COMPUTER, Kind::Computer),
    (key::POINTER_SPEED, Kind::Speed),
    (key::CONNECTED, Kind::Bool),
    (key::SHORTCUT_ONLY, Kind::Bool),
    (key::DISCOVERY, Kind::Bool),
    (key::SHARE_CLIPBOARD, Kind::Bool),
    (key::SHARE_FILES, Kind::Bool),
    (key::FILES_KEEP, Kind::Bool),
    (key::FILES_FOLDER, Kind::Folder),
];

pub fn settings() -> Value {
    let cfg = config::load();
    let mut out = Map::new();
    for (key, _) in EDITABLE {
        out.insert((*key).to_string(), cfg.0.get(*key).cloned().unwrap_or(Value::Null));
    }
    json!({
        "settings": out,
        "paired_with": cfg.str(key::SERVER_NAME),
        "status": status::read(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn check(kind: &Kind, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match kind {
        Kind::Role => match text {
            "control" => Ok(json!("control")),
            "controlled" => Ok(json!("controlled")),
            _ => Err("control or controlled".into()),
        },
        Kind::Computer => {
            if crate::discovery::clean_id(text) == text {
                Ok(json!(text))
            } else {
                Err("one of the computers Flow is paired with".into())
            }
        }
        Kind::Speed => match SPEEDS.iter().find(|(word, _)| word.eq_ignore_ascii_case(text)) {
            Some((_, n)) => Ok(json!(n)),
            None => match text.parse::<f64>() {
                Ok(n) if (0.25..=4.0).contains(&n) => Ok(json!(n)),
                _ => Err("Slower, Normal, Faster or Much faster".into()),
            },
        },
        Kind::Side => match crate::capture::Side::parse(text) {
            Some(side) => Ok(json!(side.name())),
            None => Err("Left, Right, Above or Below".into()),
        },
        Kind::Bool => match text {
            "true" | "on" | "yes" => Ok(json!(true)),
            "false" | "off" | "no" => Ok(json!(false)),
            _ => Err("on or off".into()),
        },
        Kind::Host => {
            let fine = text.len() <= 253
                && text.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]_".contains(c));
            if fine {
                Ok(json!(text))
            } else {
                Err("a computer name or address, like 192.168.1.20".into())
            }
        }
        Kind::Folder => {
            if text.is_empty() || (text.starts_with('/') && !text.contains('\0')) {
                Ok(json!(text))
            } else {
                Err("a full folder path starting with /, or empty for Downloads/Flow".into())
            }
        }
    }
}

pub fn set(key: &str, text: &str) -> Result<(), String> {
    let value = checked(key, text)?;
    let _held = config::edit_lock();
    let mut cfg = config::load();
    cfg.set(key, value);
    config::save(&cfg).map_err(|e| format!("Flow could not save the settings: {e}"))
}

pub fn checked(key: &str, text: &str) -> Result<Value, String> {
    let Some((_, kind)) = EDITABLE.iter().find(|(k, _)| *k == key) else {
        return Err(format!("Flow has no setting called '{key}'."));
    };
    check(kind, text).map_err(|want| format!("{key} must be {want}."))
}

pub fn forget() -> Result<(), String> {
    let _held = config::edit_lock();
    let mut cfg = config::load();
    crate::pairing::unpair_into(&mut cfg);
    config::save(&cfg).map_err(|e| format!("Flow could not save the settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TestHome;

    #[test]
    fn values_are_checked() {
        assert_eq!(check(&Kind::Bool, "on"), Ok(json!(true)));
        assert!(check(&Kind::Bool, "maybe").is_err());
        assert_eq!(check(&Kind::Speed, "Faster"), Ok(json!(1.5)));
        assert_eq!(check(&Kind::Speed, "0.8"), Ok(json!(0.8)));
        assert!(check(&Kind::Speed, "9").is_err());
        assert!(check(&Kind::Computer, "my_pc-2").is_ok());
        assert!(!EDITABLE.iter().any(|(k, _)| *k == "port"));
        assert!(check(&Kind::Host, "evil;rm").is_err());
        assert_eq!(check(&Kind::Host, "192.168.1.20"), Ok(json!("192.168.1.20")));
        assert!(check(&Kind::Folder, "relative/path").is_err());
        assert_eq!(check(&Kind::Role, "control"), Ok(json!("control")));
        assert_eq!(check(&Kind::Role, "controlled"), Ok(json!("controlled")));
        assert!(check(&Kind::Role, "both").is_err());
        assert_eq!(check(&Kind::Side, "Above"), Ok(json!("top")));
        assert_eq!(check(&Kind::Side, "Left"), Ok(json!("left")));
        assert!(check(&Kind::Side, "behind").is_err());
    }

    #[test]
    fn settings_never_show_the_key() {
        let _home = TestHome::new();
        let token = config::load().str(key::TOKEN);
        assert!(!token.is_empty());
        assert!(!settings().to_string().contains(&token));
        assert!(set(key::TOKEN, "x").is_err());
        assert!(set(key::SHARE_FILES, "off").is_ok());
        assert!(!config::load().bool(key::SHARE_FILES));
    }

    #[test]
    fn forgetting_unpairs_and_changes_the_key() {
        let _home = TestHome::new();
        config::set_value(key::SERVER_NAME, json!("John's laptop"));
        let token = config::load().str(key::TOKEN);
        assert!(forget().is_ok());
        let cfg = config::load();
        assert_eq!(cfg.str(key::SERVER_NAME), "");
        assert_ne!(cfg.str(key::TOKEN), token);
    }
}
