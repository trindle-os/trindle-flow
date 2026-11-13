//! Every message the help page quotes is one Flow really says, so the quotes cannot drift from the code.

use std::fs;
use std::path::Path;

// Said by the locked computer's Trindle, not by Flow.
const FROM_TRINDLE: [&str; 5] = ["Not locked.", "No PIN set.", "PIN time is over.", "Wrong PIN.", "Too many tries."];

fn sources(dir: &Path, all: &mut String) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, all);
        } else if path.extension().is_some_and(|e| e == "rs") {
            all.push_str(&fs::read_to_string(&path).unwrap());
        }
    }
}

fn flat(text: &str) -> String {
    text.replace("\\\n", "\n").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn quotes(help: &str) -> Vec<String> {
    let start = help.find("## When something goes wrong").expect("the help has a troubleshooting part");
    let part = &help[start..];
    let part = &part[..part[3..].find("\n## ").map_or(part.len(), |i| i + 3)];
    let text = flat(part);
    let mut found = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let quote = &after[..close];
        if quote.ends_with('.') && quote.len() > 12 && !FROM_TRINDLE.contains(&quote) {
            found.push(quote.to_string());
        }
        rest = &after[close + 1..];
    }
    found
}

#[test]
fn help_quotes_what_flow_says() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut code = String::new();
    sources(&root.join("src"), &mut code);
    let code = flat(&code);
    let help = fs::read_to_string(root.join("docs/HELP.md")).unwrap();
    let quoted = quotes(&help);
    assert!(quoted.len() >= 8, "found only {} quoted messages", quoted.len());
    let missing: Vec<&String> = quoted
        .iter()
        .filter(|q| !q.split("...").map(str::trim).filter(|p| !p.is_empty()).all(|p| code.contains(p)))
        .collect();
    assert!(missing.is_empty(), "the help quotes messages Flow does not say: {missing:?}");
}
