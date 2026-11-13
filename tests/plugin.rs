//! The Claude Code plugin (`skills/` as the plugin folder) and the
//! repository's marketplace file stay loadable and agree with each other.

use serde_json::Value;
use std::path::Path;

fn json(rel: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_plugin_and_its_marketplace_entry_agree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin = json("skills/.claude-plugin/plugin.json");
    let market = json(".claude-plugin/marketplace.json");
    let entry = &market["plugins"][0];

    // An install is `<entry name>@<marketplace>`; skills are prefixed with
    // the manifest name. Different names make the plugin unfindable.
    assert_eq!(plugin["name"], "navigera");
    assert_eq!(entry["name"], plugin["name"]);

    let source = entry["source"].as_str().expect("a relative source");
    let folder = root.join(source);
    assert!(folder.join(".claude-plugin/plugin.json").is_file(), "{source} holds no plugin");

    // The directory lists nothing without a README of 40 words and a license.
    let readme = std::fs::read_to_string(folder.join("README.md")).expect("plugin README");
    assert!(readme.split_whitespace().count() >= 40);
    assert_eq!(plugin["license"], env!("CARGO_PKG_LICENSE"));

    // Every skills path is inside the plugin folder and holds the skill.
    for path in plugin["skills"].as_array().expect("skills") {
        let path = path.as_str().unwrap();
        assert!(path.starts_with("./") && !path.contains(".."), "{path}");
        assert!(folder.join(path).join("navigera/SKILL.md").is_file(), "{path}");
    }

    // No component that runs code: the plugin is text only (its README says so).
    for key in ["hooks", "mcpServers", "lspServers", "commands", "agents"] {
        assert!(plugin.get(key).is_none(), "{key} in plugin.json");
    }
    for name in ["hooks", "bin", ".mcp.json", ".lsp.json"] {
        assert!(!folder.join(name).exists(), "{name} in the plugin folder");
    }
}
