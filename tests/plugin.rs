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

    // A plugin with a version stays at it until the version changes, and
    // its skill must match the binary the skill installs (the latest
    // release): the release commit bumps both.
    assert_eq!(
        plugin["version"],
        env!("CARGO_PKG_VERSION"),
        "bump `version` in skills/.claude-plugin/plugin.json with Cargo.toml"
    );

    // Every skills path is inside the plugin folder and holds the skill.
    for path in plugin["skills"].as_array().expect("skills") {
        let path = path.as_str().unwrap();
        assert!(path.starts_with("./") && !path.contains(".."), "{path}");
        assert!(folder.join(path).join("navigera/SKILL.md").is_file(), "{path}");
    }

    // The directory reads the listing's icon from this key, a path inside
    // the plugin folder; without the key the listing has no icon.
    let icon = plugin["icon"].as_str().expect("an `icon` path in plugin.json");
    assert!(icon.starts_with("./") && !icon.contains(".."), "{icon}");
    assert!(folder.join(icon).is_file(), "{icon} is not a file in the plugin folder");

    // No component that runs code: the plugin is text only (its README says so).
    for key in ["hooks", "mcpServers", "lspServers", "commands", "agents"] {
        assert!(plugin.get(key).is_none(), "{key} in plugin.json");
    }
    for name in ["hooks", "bin", ".mcp.json", ".lsp.json"] {
        assert!(!folder.join(name).exists(), "{name} in the plugin folder");
    }
}

/// The icon is stored as the bytes it was drawn as: `skills/**` is text
/// with LF endings for the files the binary embeds, and that rule once
/// rewrote the bytes of a PNG.
#[test]
fn the_plugin_icon_is_a_whole_square_png() {
    let plugin = json("skills/.claude-plugin/plugin.json");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills").join(plugin["icon"].as_str().unwrap());
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG: was it stored as text?");
    let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let (width, height) = (be(16), be(20));
    assert_eq!(width, height);
    assert!((512..=2048).contains(&width), "the plugin directory takes 512 to 2048 px, this is {width}");
    assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
}
