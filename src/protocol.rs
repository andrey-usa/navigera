//! `navigera` — the Node-free realtime browser CLI library.
//!
//! This is the library behind the `navigera` binary: a Chrome-first,
//! line-oriented JSON protocol on a from-scratch CDP engine. The binary is a
//! thin wrapper — all parsing, protocol and session logic lives here so it
//! is reusable and testable.
//!
//! ```text
//! # Agents: one warm browser behind a named session, one command per step.
//! navigera --session s start
//! navigera --session s goto https://example.com
//! navigera --session s ax                 # text tree with [ref=N]
//! navigera --session s click 12           # act on a ref
//!
//! # Programs: one JSON command per stdin line.
//! navigera serve
//! {"id":1,"op":"goto","url":"https://example.com"}
//! {"id":2,"op":"eval","expression":"() => document.title"}
//! {"id":3,"op":"quit"}
//! ```

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::time::Instant;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::browser::BrowserSession;
use crate::cdp::{AxOptions, Target};

/// Default navigation / command timeout when a command carries none of its own.
pub const DEFAULT_TIMEOUT_MS: f64 = 35_000.0;

/// Default `ax` line budget (a 500-card listing is ~1500 lines).
pub const DEFAULT_AX_LIMIT: usize = 2000;

/// The skill shipped inside the binary (`navigera skill`,
/// `navigera install-skill`), so docs always match the binary version.
pub const SKILL_MD: &str = include_str!("../skills/navigera/SKILL.md");

/// Accepts a ref as a number or as `"12"`, `"@12"`, `"ref=12"`, `"e12"`
/// (the forms other agent tools print).
fn de_ref<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            n.as_u64().map(Some).ok_or_else(|| serde::de::Error::custom("ref must be a positive integer"))
        }
        Some(Value::String(s)) => parse_ref(&s)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("bad ref {s:?}; use the number from [ref=N]"))),
        Some(other) => Err(serde::de::Error::custom(format!("bad ref {other}"))),
    }
}

/// `"12"`, `"@12"`, `"ref=12"`, `"[ref=12]"`, `"e12"` -> 12.
pub fn parse_ref(raw: &str) -> Option<u64> {
    let s = raw.trim().trim_start_matches('[').trim_end_matches(']');
    let s = s.strip_prefix("ref=").unwrap_or(s);
    let s = s.strip_prefix('@').unwrap_or(s);
    let s = s.strip_prefix('e').unwrap_or(s);
    s.parse::<u64>().ok()
}

/// A string or a list of strings (`select` values).
fn de_values<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s]),
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|v| match v {
                Value::String(s) => Ok(s),
                other => Ok(other.to_string()),
            })
            .collect(),
        Some(other) => Ok(vec![other.to_string()]),
    }
}

/// Which element an op acts on: exactly one of these.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Clone)]
pub struct TargetArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "de_ref", skip_serializing_if = "Option::is_none")]
    pub node_ref: Option<u64>,
    /// Visible text / label of the element (best match, actionable first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl TargetArgs {
    fn target(&self, op: &str) -> anyhow::Result<Target> {
        self.optional(op)?.ok_or_else(|| {
            anyhow::anyhow!(
                "{op} needs a target: a ref from `ax` (`{op} 12`), --selector <css> or --text <visible text>"
            )
        })
    }

    fn optional(&self, op: &str) -> anyhow::Result<Option<Target>> {
        let given =
            [self.selector.is_some(), self.node_ref.is_some(), self.text.is_some()].iter().filter(|b| **b).count();
        if given > 1 {
            anyhow::bail!("{op}: give only one of ref, --selector, --text");
        }
        Ok(if let Some(r) = self.node_ref {
            Some(Target::Ref(r))
        } else if let Some(s) = &self.selector {
            Some(Target::Selector(s.clone()))
        } else {
            self.text.clone().map(Target::Text)
        })
    }
}

/// One command from the model, either a CLI subcommand or a serve-mode line.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op")]
pub enum Command {
    /// Open the browser (serve mode only; one-shots launch implicitly).
    #[serde(rename = "open")]
    Open {
        #[serde(default)]
        url: Option<String>,
    },
    /// Navigate the live page.
    #[serde(rename = "goto", alias = "navigate")]
    Goto {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    #[serde(rename = "back")]
    Back {},
    #[serde(rename = "forward")]
    Forward {},
    #[serde(rename = "reload")]
    Reload {},
    /// Trusted mouse click on the target.
    #[serde(rename = "click")]
    Click {
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Set an input's value (native setter + input/change events).
    #[serde(rename = "fill")]
    Fill {
        #[serde(flatten)]
        target: TargetArgs,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Move the mouse over the target (hover menus, tooltips).
    #[serde(rename = "hover")]
    Hover {
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Choose `<select>` option(s) by value or label.
    #[serde(rename = "select")]
    Select {
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, alias = "values", deserialize_with = "de_values")]
        value: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Press a key or chord (`Enter`, `Control+a`), optionally on a target.
    #[serde(rename = "press")]
    Press {
        key: String,
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Type text key by key (into the target, or the focused element).
    #[serde(rename = "type")]
    Type {
        value: String,
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Scroll the window (`by` px, or `to` top|bottom) or a target into view.
    #[serde(rename = "scroll")]
    Scroll {
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Set the files of an `<input type=file>`.
    #[serde(rename = "upload")]
    Upload {
        #[serde(flatten)]
        target: TargetArgs,
        #[serde(default, alias = "file", deserialize_with = "de_values")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Wait until every given condition holds.
    #[serde(rename = "wait")]
    Wait {
        /// A visible element matching this CSS selector.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        /// This text anywhere in the page's visible text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// The URL contains this substring.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// No visible element matches this selector any more.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gone: Option<String>,
        /// A JS expression that becomes truthy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        js: Option<String>,
        /// Just sleep this long.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// How `alert`/`confirm`/`prompt` dialogs are answered from now on.
    #[serde(rename = "dialog")]
    Dialog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accept: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_text: Option<String>,
    },
    /// First element's textContent for the selector.
    #[serde(rename = "text")]
    Text {
        selector: String,
        #[serde(default)]
        timeout_ms: Option<f64>,
    },
    /// Evaluate a JS expression; result must be JSON-serializable.
    #[serde(rename = "eval", alias = "evaluate")]
    Eval {
        expression: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Page title.
    #[serde(rename = "title")]
    Title {},
    /// Accessibility snapshot: an indented text tree with `[ref=N]` on
    /// actionable nodes (iframes included). `format: "json"` returns the
    /// older flat `[{ref, role, name, value?}]` list (`all: true` = raw).
    #[serde(rename = "ax", alias = "snapshot")]
    Ax {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        all: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        /// `"all"`: a ref on every node, not just actionable ones.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refs: Option<String>,
        /// Only the subtree of this element.
        #[serde(flatten)]
        scope: TargetArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    /// Current page URL and tab state.
    #[serde(rename = "url")]
    Url {},
    /// PNG screenshot to disk; result is the written path + byte length.
    #[serde(rename = "screenshot")]
    Screenshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full_page: Option<bool>,
    },
    /// List open tabs: index, target id, and whether it is the active tab.
    #[serde(rename = "tab-list")]
    TabList {},
    /// Open a fresh tab (optionally at a URL) and make it active.
    #[serde(rename = "tab-new")]
    TabNew {
        #[serde(default)]
        url: Option<String>,
    },
    /// Switch the active tab by zero-based index.
    #[serde(rename = "tab-select")]
    TabSelect { index: usize },
    /// Close one tab by index (defaults to the active tab).
    #[serde(rename = "tab-close")]
    TabClose {
        #[serde(default)]
        index: Option<usize>,
    },
    /// Alias of `tab-close`.
    #[serde(rename = "close-page")]
    ClosePage {
        #[serde(default)]
        index: Option<usize>,
    },
    /// Close the session and exit (serve mode). An attached session
    /// (`--attach`, `--profile`) only disconnects, unless `close_browser`.
    #[serde(rename = "quit", alias = "close")]
    Quit {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        close_browser: bool,
    },
}

/// JSON response for one command.
#[derive(Debug, Serialize, PartialEq)]
pub struct Response {
    pub id: Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Dialogs the page opened during this op and how they were answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialogs: Option<Vec<Value>>,
    /// Tabs the page opened during this op (now in the tab list; the last
    /// one is active).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_tabs: Option<Vec<usize>>,
    /// Server-side time for the op, in milliseconds with µs resolution.
    pub elapsed_ms: f64,
}

/// Launch/session settings shared by one-shot and serve mode.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// Browser engine (`chrome` by default — the validated production path).
    pub engine: String,
    pub headed: bool,
    pub chromium: Option<String>,
    pub timeout_ms: f64,
    pub pretty: bool,
    /// Print only the result (strings raw), errors as `error: …` on stderr.
    pub raw: bool,
    /// CDP transport: `pipe` (default on Linux/macOS) or `ws` (DevTools
    /// port; the default on Windows).
    pub transport: Option<String>,
    /// Named background session (`--session <name|socket path>`): `serve`
    /// listens on it, `start` spawns that server detached, and every other
    /// command becomes a client call against the warm browser behind it.
    pub session: Option<String>,
    /// A session server with no traffic for this long shuts its browser down
    /// (0 = never). Keeps forgotten agent sessions from leaking Chrome.
    pub idle_timeout_s: u64,
    /// `--attach <where>`: work in a browser that is already running
    /// (default `chrome`: the user's own, via chrome://inspect remote
    /// debugging) instead of launching one. Never closes it.
    pub attach: Option<String>,
    /// `--profile <name|dir>`: a persistent, visible navigera browser on
    /// its own profile, started when needed and kept open between sessions.
    pub profile: Option<String>,
    /// `--headless` for a `--profile` browser (visible by default).
    pub headless: bool,
}

/// Before `ax`/`screenshot`: no fetch/XHR for this long (Playwright's
/// `networkidle` uses the same 500 ms) …
const NETWORK_QUIET: std::time::Duration = std::time::Duration::from_millis(500);
/// … waiting at most this long (long polls, chatty analytics).
const NETWORK_QUIET_MAX: std::time::Duration = std::time::Duration::from_secs(3);

/// Default idle shutdown for session servers.
pub const DEFAULT_IDLE_TIMEOUT_S: u64 = 1800;

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            engine: "chrome".to_string(),
            headed: false,
            chromium: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            pretty: false,
            raw: false,
            transport: None,
            session: None,
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
            attach: None,
            profile: None,
            headless: false,
        }
    }
}

impl SessionConfig {
    /// The browser to connect to instead of launching one, if any.
    pub fn attach_target(&self) -> Option<crate::browser::AttachTarget> {
        if let Some(spec) = &self.attach {
            return Some(crate::browser::AttachTarget::Running(spec.clone()));
        }
        self.profile.as_ref().map(|name| crate::browser::AttachTarget::Profile {
            name: name.clone(),
            executable: self.chromium.clone(),
            headless: self.headless,
        })
    }
}

/// `--attach` takes an optional value; the default is the user's Chrome.
const DEFAULT_ATTACH: &str = "chrome";

/// True when `word` is a command name (so it can't be `--attach`'s value).
fn is_command_word(word: &str) -> bool {
    matches!(word, "serve" | "skill" | "install-skill" | "help") || OPS.iter().any(|(name, _, _)| *name == word)
}

/// Pull `--attach [value]` out of argv; a bare `--attach` means `chrome`.
fn extract_attach(args: &mut std::collections::VecDeque<String>) -> Result<Option<String>, String> {
    let mut found = None;
    let mut rest = std::collections::VecDeque::new();
    while let Some(arg) = args.pop_front() {
        let value = if arg == "--attach" {
            match args.front() {
                Some(next) if !next.starts_with('-') && !is_command_word(next) => args.pop_front(),
                _ => Some(DEFAULT_ATTACH.to_string()),
            }
        } else if let Some(v) = arg.strip_prefix("--attach=") {
            Some(v.to_string())
        } else {
            rest.push_back(arg);
            continue;
        };
        if found.is_some() {
            return Err("duplicate --attach".into());
        }
        found = value;
    }
    *args = rest;
    Ok(found)
}

/// Commands that never touch a browser.
#[derive(Debug, PartialEq)]
pub enum Local {
    /// Print the embedded SKILL.md.
    Skill,
    /// Write SKILL.md to `<dir>/navigera/SKILL.md`.
    InstallSkill {
        dir: String,
    },
    Version,
}

/// Result of [`parse_args`]: launch config plus an optional one-shot command.
#[derive(Debug, PartialEq)]
pub struct ParsedArgs {
    pub config: SessionConfig,
    /// `None` selects serve mode (stdin/stdout protocol, or the socket when
    /// `config.session` is set).
    pub command: Option<Command>,
    /// `start`: spawn a detached session server for `config.session`.
    pub start: bool,
    /// A command that runs without a browser.
    pub local: Option<Local>,
}

/// One op's CLI documentation: (name, synopsis, what it does).
const OPS: &[(&str, &str, &str)] = &[
    ("start", "[--attach [<where>] | --profile <name> [--headless]]", "spawn a detached session server + headless browser (needs --session); --attach/--profile use a browser that stays open"),
    ("goto", "<url> [--wait load|domcontentloaded|commit]", "navigate the active tab (default: wait for load)"),
    ("ax", "[--selector <css> | <ref>] [--limit <lines>] [--refs all] [--format json]", "accessibility snapshot: indented tree, [ref=N] on actionable nodes"),
    ("click", "<ref> | --selector <css> | --text <visible text>", "trusted mouse click; waits for the element and for any navigation it starts"),
    ("fill", "<ref> <value> | --selector <css> --value <v>", "set an input/textarea's value"),
    ("type", "<text> [--selector <css> | --ref <n>]", "type key by key into the target or the focused element"),
    ("press", "<key> [--selector <css> | --ref <n>]", "press a key or chord: Enter, Tab, Escape, ArrowDown, Control+a"),
    ("select", "<ref> <option> | --selector <css> --value <option>", "choose <select> option(s) by value or label"),
    ("hover", "<ref> | --selector <css> | --text <t>", "move the mouse over an element (hover menus)"),
    ("scroll", "[--by <px> | --to top|bottom] [<ref> | --selector <css>]", "scroll the page, or an element into view"),
    ("upload", "<ref> <file>... | --selector <css> --file <path>", "set the files of an <input type=file>"),
    ("wait", "[--selector <css>] [--text <t>] [--url <part>] [--gone <css>] [--js <expr>] [--ms <n>]", "wait until every condition holds (default up to 5 s; --timeout-ms)"),
    ("eval", "<js> | --file <path.js> | -", "evaluate JS (arrow functions are called); prints the JSON result; `-` reads the script from stdin"),
    ("text", "--selector <css>", "textContent of the first match"),
    ("title", "", "page title"),
    ("url", "", "active tab state {tab, tabs, url, title}"),
    ("back", "", "history back"),
    ("forward", "", "history forward"),
    ("reload", "", "reload the page"),
    ("screenshot", "[--path <f.png>] [--full-page]", "PNG of the viewport (or whole page)"),
    ("tab-new", "[<url>]", "open a tab and make it active"),
    ("tab-list", "", "list tabs"),
    ("tab-select", "<index>", "switch the active tab"),
    ("tab-close", "[<index>]", "close a tab (default: active)"),
    ("dialog", "--accept [--prompt-text <t>] | --dismiss", "how alert/confirm/prompt are answered (default: accept; every dialog is reported)"),
    ("quit", "[--close-browser]", "close the browser and the session server; attached (--attach/--profile): only disconnect, unless --close-browser"),
    ("skill", "", "print the agent skill (usage guide) for this version"),
    ("install-skill", "[--dir <skills dir>] [--claude] [--global]", "install the skill for agents (default ./.agents/skills; --claude: ./.claude/skills)"),
];

/// Help text for `--help` / argument errors.
pub fn usage(program: &str) -> String {
    let mut out = format!(
        "navigera {} — drive headless Chrome over CDP, one shell command per step.\n\n\
         AGENT LOOP:\n  \
         {program} --session s start\n  \
         {program} --session s goto https://example.com\n  \
         {program} --session s ax                     # read: tree with [ref=N]\n  \
         {program} --session s click 12               # act on a ref\n  \
         {program} --session s fill 31 \"hello\"\n  \
         {program} --session s quit\n\n\
         COMMANDS:\n",
        env!("CARGO_PKG_VERSION")
    );
    for (name, args, what) in OPS {
        out.push_str(&format!("  {name:<13} {args}\n  {:<13} {what}\n", ""));
    }
    out.push_str(&format!(
        "\nGLOBAL FLAGS (before or after the command):\n  \
         --session <name>       warm browser behind a local socket ($NAVIGERA_SESSION)\n  \
         --raw                  print only the result (strings unquoted); errors on stderr\n  \
         --timeout-ms <ms>      navigation/command timeout (default {DEFAULT_TIMEOUT_MS}); per op: element wait (default 5000)\n  \
         --chromium <path>      browser binary ($CHROME_BIN); --engine lightpanda; --headed\n  \
         --transport ws         CDP over a DevTools port instead of a private pipe (to attach DevTools; Windows default)\n  \
         --idle-timeout-s <s>   session server idle shutdown (default {DEFAULT_IDLE_TIMEOUT_S}, 0 = never)\n  \
         --attach [chrome|edge|<dir>|<port>]  with `start`: use your running browser (Chrome 144+: chrome://inspect/#remote-debugging); never closes it\n  \
         --profile <name>       with `start`: persistent visible navigera browser, kept open between sessions (--headless to hide)\n  \
         --pretty               pretty-print JSON\n\n\
         Output: one JSON object per command, {{\"ok\":true,\"result\":…}} or {{\"ok\":false,\"error\":…}}; exit 1 on error.\n\
         Without --session each command launches and closes its own browser (one-shot).\n\
         Programs: `{program} serve` reads JSON commands on stdin, e.g. {{\"id\":1,\"op\":\"goto\",\"url\":\"…\"}}.\n\
         More: `{program} help <command>`, `{program} skill`."
    ));
    out
}

/// `help <op>`.
pub fn op_help(program: &str, op: &str) -> Option<String> {
    let (name, args, what) = OPS.iter().find(|(n, ..)| *n == op)?;
    Some(format!("usage: {program} [--session <s>] {name} {args}\n{what}"))
}

/// Nearest known command for a typo, plus aliases other agent CLIs use.
fn suggest(op: &str) -> Option<&'static str> {
    let aliases: &[(&str, &str)] = &[
        ("open-url", "goto"),
        ("visit", "goto"),
        ("go", "goto"),
        ("evaluate", "eval"),
        ("js", "eval"),
        ("exec", "eval"),
        ("key", "press"),
        ("keypress", "press"),
        ("check", "click"),
        ("tap", "click"),
        ("dblclick", "click"),
        ("input", "fill"),
        ("clear", "fill"),
        ("tabs", "tab-list"),
        ("tab", "tab-list"),
        ("exit", "quit"),
        ("stop", "quit"),
        ("tree", "ax"),
        ("a11y", "ax"),
        ("accessibility", "ax"),
        ("refresh", "reload"),
        ("sleep", "wait"),
        ("waitfor", "wait"),
        ("shot", "screenshot"),
    ];
    if let Some((_, to)) = aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(op)) {
        return Some(to);
    }
    fn dist(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut cur = vec![i + 1];
            for (j, cb) in b.iter().enumerate() {
                cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
            }
            prev = cur;
        }
        prev[b.len()]
    }
    let extra: &[(&str, &str)] = &[("navigate", "goto"), ("snapshot", "ax"), ("evaluate", "eval"), ("close", "quit")];
    OPS.iter()
        .map(|(n, ..)| (*n, *n))
        .chain(aliases.iter().chain(extra.iter()).copied())
        .map(|(name, to)| (to, dist(&op.to_ascii_lowercase(), name)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(n, _)| n)
}

fn next_value(args: &mut std::collections::VecDeque<String>, flag: &str) -> Result<String, String> {
    args.pop_front().ok_or_else(|| format!("missing value after {flag}"))
}

/// Why [`parse_args`] failed: `Help` is a deliberate `--help` (exit 0),
/// `Invalid` is bad usage (exit 2).
#[derive(Debug, PartialEq)]
pub enum ArgsError {
    /// Usage text, requested explicitly (`--help`) or because no args given.
    Help(String),
    /// Invalid invocation; message may embed the usage text.
    Invalid(String),
}

fn parse_f64(raw: &str, flag: &str) -> Result<f64, String> {
    raw.parse::<f64>().map_err(|_| format!("{flag} must be a number, got {raw:?}"))
}

fn parse_index(raw: &str) -> Result<usize, String> {
    raw.parse::<usize>().map_err(|_| format!("tab index must be a number, got {raw:?}"))
}

/// Scan every `--flag value` (or `--flag=value`) out of the remaining argv.
fn extract_values(args: &mut std::collections::VecDeque<String>, flag: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut rest = std::collections::VecDeque::new();
    let with_eq = format!("{flag}=");
    while let Some(arg) = args.pop_front() {
        if arg == flag {
            values.push(next_value(args, flag)?);
        } else if let Some(v) = arg.strip_prefix(&with_eq) {
            values.push(v.to_string());
        } else {
            rest.push_back(arg);
        }
    }
    *args = rest;
    Ok(values)
}

/// Scan one `--flag value` out of the remaining argv (errors on duplicates).
fn extract_value(args: &mut std::collections::VecDeque<String>, flag: &str) -> Result<Option<String>, String> {
    let mut values = extract_values(args, flag)?;
    if values.len() > 1 {
        return Err(format!("duplicate {flag}"));
    }
    Ok(values.pop())
}

/// Scan one boolean `--flag` out of the remaining argv.
fn extract_present(args: &mut std::collections::VecDeque<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// Global flags may appear before *or* after the command (`eval … --pretty`).
fn consume_globals(args: &mut std::collections::VecDeque<String>, config: &mut SessionConfig) -> Result<(), String> {
    if let Some(raw) = extract_value(args, "--engine")? {
        config.engine = raw;
    }
    if extract_present(args, "--headed") {
        config.headed = true;
    }
    if let Some(path) = extract_value(args, "--chromium")? {
        config.chromium = Some(path);
    }
    if extract_present(args, "--pretty") {
        config.pretty = true;
    }
    if extract_present(args, "--raw") {
        config.raw = true;
    }
    if let Some(t) = extract_value(args, "--transport")? {
        config.transport = Some(t);
    }
    if let Some(name) = extract_value(args, "--session")?.or(extract_value(args, "-s")?) {
        config.session = Some(name);
    }
    if let Some(raw) = extract_value(args, "--idle-timeout-s")? {
        config.idle_timeout_s =
            raw.parse::<u64>().map_err(|_| format!("--idle-timeout-s must be whole seconds, got {raw:?}"))?;
    }
    if let Some(spec) = extract_attach(args)? {
        config.attach = Some(spec);
    }
    if let Some(name) = extract_value(args, "--profile")? {
        config.profile = Some(name);
    }
    if extract_present(args, "--headless") {
        config.headless = true;
    }
    if config.attach.is_some() && config.profile.is_some() {
        return Err("--attach and --profile are alternatives: pick one".into());
    }
    if (config.attach.is_some() || config.profile.is_some()) && crate::browser::is_lightpanda(&config.engine) {
        return Err("--attach/--profile drive Chrome; drop --engine lightpanda".into());
    }
    Ok(())
}

type Args = std::collections::VecDeque<String>;

fn invalid<T>(msg: impl Into<String>) -> Result<T, ArgsError> {
    Err(ArgsError::Invalid(msg.into()))
}

fn str_flag(args: &mut Args, flag: &str) -> Result<Option<String>, ArgsError> {
    extract_value(args, flag).map_err(ArgsError::Invalid)
}

fn num_flag(args: &mut Args, flag: &str) -> Result<Option<f64>, ArgsError> {
    match str_flag(args, flag)? {
        Some(raw) => Ok(Some(parse_f64(&raw, flag).map_err(ArgsError::Invalid)?)),
        None => Ok(None),
    }
}

/// `--selector` / `--ref` / `--text` flags, or a leading positional: a ref
/// (`12`, `@12`, `e12`, `ref=12`) when it parses as one, else a selector.
fn target_args(args: &mut Args, positional_target: bool) -> Result<TargetArgs, ArgsError> {
    let mut t = TargetArgs {
        selector: str_flag(args, "--selector")?,
        node_ref: match str_flag(args, "--ref")? {
            Some(raw) => Some(parse_ref(&raw).ok_or_else(|| {
                ArgsError::Invalid(format!("--ref must be a number from [ref=N] in `ax`, got {raw:?}"))
            })?),
            None => None,
        },
        text: str_flag(args, "--text")?,
    };
    if positional_target && t == TargetArgs::default() {
        if let Some(first) = args.front().filter(|a| !a.starts_with("--")).cloned() {
            args.pop_front();
            match parse_ref(&first) {
                Some(r) => t.node_ref = Some(r),
                None => t.selector = Some(first),
            }
        }
    }
    Ok(t)
}

fn positional(args: &mut Args) -> Option<String> {
    if args.front().is_some_and(|a| !a.starts_with("--")) {
        args.pop_front()
    } else {
        None
    }
}

/// Parse a full argv (`argv[0]` = program name) into a session config and,
/// for one-shot invocation, the single command to run.
///
/// `Err(ArgsError::Help)` means usage was requested (`--help`, `help`, or no
/// arguments at all); every other failure is `ArgsError::Invalid`.
pub fn parse_args<I: IntoIterator<Item = String>>(argv: I) -> Result<ParsedArgs, ArgsError> {
    let mut argv = argv.into_iter();
    let program = argv.next().unwrap_or_else(|| "navigera".to_string());
    let program =
        std::path::Path::new(&program).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(program);
    let mut args: Args = argv.collect();
    let mut config = SessionConfig::default();

    // Leading globals (the only place --timeout-ms is global; after a
    // command it is that op's own timeout).
    while args.front().is_some_and(|a| a.starts_with('-')) {
        let flag = args.pop_front().expect("front checked");
        let (flag, inline) = match flag.split_once('=') {
            Some((f, v)) if f.starts_with("--") || f == "-s" => (f.to_string(), Some(v.to_string())),
            _ => (flag, None),
        };
        let mut value = |name: &str| -> Result<String, ArgsError> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => next_value(&mut args, name).map_err(ArgsError::Invalid),
            }
        };
        match flag.as_str() {
            "--engine" => config.engine = value("--engine")?,
            "--headed" => config.headed = true,
            "--chromium" => config.chromium = Some(value("--chromium")?),
            "--timeout-ms" => {
                let raw = value("--timeout-ms")?;
                config.timeout_ms = parse_f64(&raw, "--timeout-ms").map_err(ArgsError::Invalid)?;
            }
            "--pretty" => config.pretty = true,
            "--raw" => config.raw = true,
            "--transport" => config.transport = Some(value("--transport")?),
            "--session" | "-s" => config.session = Some(value("--session")?),
            "--attach" => {
                config.attach = Some(match &inline {
                    Some(v) => v.clone(),
                    None => match args.front() {
                        Some(next) if !next.starts_with('-') && !is_command_word(next) => {
                            args.pop_front().expect("front checked")
                        }
                        _ => DEFAULT_ATTACH.to_string(),
                    },
                })
            }
            "--profile" => config.profile = Some(value("--profile")?),
            "--headless" => config.headless = true,
            "--idle-timeout-s" => {
                let raw = value("--idle-timeout-s")?;
                config.idle_timeout_s = raw
                    .parse::<u64>()
                    .map_err(|_| ArgsError::Invalid(format!("--idle-timeout-s must be whole seconds, got {raw:?}")))?;
            }
            "-V" | "--version" => {
                return Ok(ParsedArgs { config, command: None, start: false, local: Some(Local::Version) })
            }
            "-h" | "--help" => return Err(ArgsError::Help(usage(&program))),
            other => {
                return invalid(format!("unknown flag {other:?} (see `{program} --help`)"));
            }
        }
    }

    let Some(op) = args.pop_front() else {
        return Err(ArgsError::Help(usage(&program)));
    };
    if op == "-h" || op == "--help" || op == "help" {
        return match args.front() {
            Some(topic) => op_help(&program, topic)
                .map(ArgsError::Help)
                .map(Err)
                .unwrap_or_else(|| invalid(format!("no command {topic:?}; see `{program} --help`"))),
            None => Err(ArgsError::Help(usage(&program))),
        };
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        if let Some(help) = op_help(&program, &op) {
            return Err(ArgsError::Help(help));
        }
    }
    consume_globals(&mut args, &mut config).map_err(ArgsError::Invalid)?;
    let parsed = |config, command, start, local| Ok(ParsedArgs { config, command, start, local });

    match op.as_str() {
        "serve" | "start" => {
            if let Some(raw) = str_flag(&mut args, "--timeout-ms")? {
                config.timeout_ms = parse_f64(&raw, "--timeout-ms").map_err(ArgsError::Invalid)?;
            }
            if let Some(extra) = args.front() {
                return invalid(format!("unexpected argument {extra:?} after {op}"));
            }
            return parsed(config, None, op == "start", None);
        }
        "skill" => return parsed(config, None, false, Some(Local::Skill)),
        "install-skill" => {
            let claude = extract_present(&mut args, "--claude");
            let global = extract_present(&mut args, "--global");
            let dir = match str_flag(&mut args, "--dir")? {
                Some(d) => d,
                None => {
                    let base = if claude { ".claude/skills" } else { ".agents/skills" };
                    if global {
                        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                        format!("{home}/{base}")
                    } else {
                        base.to_string()
                    }
                }
            };
            return parsed(config, None, false, Some(Local::InstallSkill { dir }));
        }
        _ => {}
    }

    let timeout_ms = num_flag(&mut args, "--timeout-ms")?;
    let need = |name: &str, what: &str| {
        ArgsError::Invalid(format!("{op} needs {what}; usage: {}", op_help(&program, name).unwrap_or_default()))
    };
    let command = match op.as_str() {
        "open" => Command::Open {
            url: match str_flag(&mut args, "--url")? {
                Some(u) => Some(u),
                None => positional(&mut args),
            },
        },
        "goto" | "navigate" => Command::Goto {
            url: match str_flag(&mut args, "--url")? {
                Some(u) => u,
                None => positional(&mut args).ok_or_else(|| need("goto", "a URL"))?,
            },
            wait: str_flag(&mut args, "--wait")?,
            timeout_ms,
        },
        "back" => Command::Back {},
        "forward" => Command::Forward {},
        "reload" => Command::Reload {},
        "click" => Command::Click { target: target_args(&mut args, true)?, timeout_ms },
        "hover" => Command::Hover { target: target_args(&mut args, true)?, timeout_ms },
        "fill" => {
            let target = target_args(&mut args, true)?;
            let value = match str_flag(&mut args, "--value")? {
                Some(v) => v,
                None => {
                    positional(&mut args).ok_or_else(|| need("fill", "a value (`fill <ref> <value>` or --value)"))?
                }
            };
            Command::Fill { target, value, timeout_ms }
        }
        "select" => {
            let target = target_args(&mut args, true)?;
            let mut value = extract_values(&mut args, "--value").map_err(ArgsError::Invalid)?;
            while let Some(v) = positional(&mut args) {
                value.push(v);
            }
            if value.is_empty() {
                return Err(need("select", "an option value or label"));
            }
            Command::Select { target, value, timeout_ms }
        }
        "press" => {
            let key = match str_flag(&mut args, "--key")? {
                Some(k) => k,
                None => positional(&mut args).ok_or_else(|| need("press", "a key (Enter, Tab, Control+a, …)"))?,
            };
            Command::Press { key, target: target_args(&mut args, false)?, timeout_ms }
        }
        "type" => {
            let value = match str_flag(&mut args, "--value")? {
                Some(v) => v,
                None => positional(&mut args).ok_or_else(|| need("type", "the text to type"))?,
            };
            Command::Type { value, target: target_args(&mut args, false)?, timeout_ms }
        }
        "scroll" => Command::Scroll {
            by: num_flag(&mut args, "--by")?,
            to: str_flag(&mut args, "--to")?,
            target: target_args(&mut args, true)?,
            timeout_ms,
        },
        "upload" => {
            let target = target_args(&mut args, true)?;
            let mut files = extract_values(&mut args, "--file").map_err(ArgsError::Invalid)?;
            while let Some(f) = positional(&mut args) {
                files.push(f);
            }
            if files.is_empty() {
                return Err(need("upload", "a file path"));
            }
            Command::Upload { target, files, timeout_ms }
        }
        "wait" => {
            let mut cmd = Command::Wait {
                selector: str_flag(&mut args, "--selector")?,
                text: str_flag(&mut args, "--text")?,
                url: str_flag(&mut args, "--url")?,
                gone: str_flag(&mut args, "--gone")?,
                js: str_flag(&mut args, "--js")?,
                ms: num_flag(&mut args, "--ms")?,
                timeout_ms,
            };
            if let (Some(p), Command::Wait { selector, ms, .. }) = (positional(&mut args), &mut cmd) {
                match p.parse::<f64>() {
                    Ok(n) => *ms = Some(n),
                    Err(_) => *selector = Some(p),
                }
            }
            if matches!(
                &cmd,
                Command::Wait { selector: None, text: None, url: None, gone: None, js: None, ms: None, .. }
            ) {
                return Err(need("wait", "a condition"));
            }
            cmd
        }
        "dialog" => {
            let accept = extract_present(&mut args, "--accept");
            let dismiss = extract_present(&mut args, "--dismiss");
            if accept == dismiss {
                return Err(need("dialog", "exactly one of --accept or --dismiss"));
            }
            Command::Dialog { accept: Some(accept), prompt_text: str_flag(&mut args, "--prompt-text")? }
        }
        "text" => Command::Text {
            selector: match str_flag(&mut args, "--selector")? {
                Some(s) => s,
                None => positional(&mut args).ok_or_else(|| need("text", "--selector <css>"))?,
            },
            timeout_ms,
        },
        "eval" | "evaluate" => {
            // `--file <path>` and `-` (stdin) keep the script away from the
            // shell's quoting rules (PowerShell mangles quotes and backslashes).
            let expression = match (str_flag(&mut args, "--file")?, str_flag(&mut args, "--expression")?) {
                (Some(path), None) => std::fs::read_to_string(&path)
                    .map_err(|e| ArgsError::Invalid(format!("eval --file {path}: {e}")))?,
                (None, Some(e)) => e,
                (Some(_), Some(_)) => return invalid("eval takes --file or an expression, not both"),
                (None, None) => match positional(&mut args) {
                    Some(dash) if dash == "-" => {
                        let mut script = String::new();
                        io::Read::read_to_string(&mut io::stdin(), &mut script)
                            .map_err(|e| ArgsError::Invalid(format!("eval -: read stdin: {e}")))?;
                        script
                    }
                    Some(e) => e,
                    None => return Err(need("eval", "a JS expression, --file <path> or - (stdin)")),
                },
            };
            Command::Eval { expression: expression.trim_start_matches('\u{feff}').to_string(), timeout_ms }
        }
        "title" => Command::Title {},
        "ax" | "snapshot" => Command::Ax {
            max_depth: num_flag(&mut args, "--max-depth")?.map(|v| v as u32),
            all: extract_present(&mut args, "--all").then_some(true),
            // The CLI defaults to the text tree; the serve protocol keeps
            // its original JSON list when `format` is absent.
            format: Some(str_flag(&mut args, "--format")?.unwrap_or_else(|| "text".into())),
            limit: num_flag(&mut args, "--limit")?.map(|v| v as usize),
            refs: str_flag(&mut args, "--refs")?,
            scope: target_args(&mut args, true)?,
            timeout_ms,
        },
        "url" => Command::Url {},
        // Mostly for sessions (`--session s quit` stops the server); as a
        // one-shot it just launches and closes.
        "quit" | "close" => Command::Quit { close_browser: extract_present(&mut args, "--close-browser") },
        "tab-list" => Command::TabList {},
        "tab-new" => Command::TabNew {
            url: match str_flag(&mut args, "--url")? {
                Some(u) => Some(u),
                None => positional(&mut args),
            },
        },
        "tab-select" => Command::TabSelect {
            index: match str_flag(&mut args, "--index")?.or_else(|| positional(&mut args)) {
                Some(raw) => parse_index(&raw).map_err(ArgsError::Invalid)?,
                None => return Err(need("tab-select", "a tab index")),
            },
        },
        "tab-close" | "close-page" => {
            let index = str_flag(&mut args, "--index")?
                .or_else(|| positional(&mut args))
                .map(|raw| parse_index(&raw))
                .transpose()
                .map_err(ArgsError::Invalid)?;
            if op == "tab-close" {
                Command::TabClose { index }
            } else {
                Command::ClosePage { index }
            }
        }
        "screenshot" => Command::Screenshot {
            path: match str_flag(&mut args, "--path")? {
                Some(p) => Some(p),
                None => positional(&mut args),
            },
            full_page: extract_present(&mut args, "--full-page").then_some(true),
        },
        unknown => {
            let hint = suggest(unknown).map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default();
            return invalid(format!(
                "unknown command {unknown:?}{hint}\ncommands: {}\n(`{program} --help` for usage)",
                OPS.iter().map(|(n, ..)| *n).collect::<Vec<_>>().join(", ")
            ));
        }
    };
    if let Some(extra) = args.front() {
        let hint = if extra.starts_with("--") {
            format!("; usage: {}", op_help(&program, &op).unwrap_or_default())
        } else {
            " (quote arguments that contain spaces)".to_string()
        };
        return invalid(format!("unexpected argument {extra:?} for `{op}`{hint}"));
    }
    parsed(config, Some(command), false, None)
}

/// Install the embedded skill into `<dir>/navigera/SKILL.md`.
pub fn install_skill(dir: &str) -> anyhow::Result<String> {
    let path = std::path::Path::new(dir).join("navigera");
    std::fs::create_dir_all(&path)?;
    let file = path.join("SKILL.md");
    std::fs::write(&file, SKILL_MD)?;
    Ok(file.display().to_string())
}

/// A live browser plus the policy defaults the model gets for free.
///
/// Construct once per session; every [`Command`] runs against the same warm
/// Chrome (no relaunch between ops).
pub struct Driver {
    session: BrowserSession,
}

/// One op's result plus side effects the agent should know about.
pub struct Outcome {
    pub result: Value,
    pub dialogs: Vec<Value>,
    pub new_tabs: Vec<usize>,
}

impl Driver {
    /// Launch the engine described by `config` (Chrome by default).
    pub fn launch(config: &SessionConfig) -> anyhow::Result<Self> {
        if let Some(target) = config.attach_target() {
            let session = BrowserSession::attach(&target, config.timeout_ms)
                .map_err(|e| anyhow::anyhow!("could not connect to the browser: {e:#}"))?;
            return Ok(Self { session });
        }
        let session = BrowserSession::launch_with(
            &config.engine,
            !config.headed,
            config.chromium.as_deref(),
            config.timeout_ms,
            config.transport.as_deref(),
        )
        .map_err(|e| {
            anyhow::anyhow!("browser launch failed (engine={}, headed={}): {e:#}", config.engine, config.headed)
        })?;
        Ok(Self { session })
    }

    /// The active page (tab) this driver's ops target.
    pub fn session(&self) -> &BrowserSession {
        &self.session
    }

    /// Active tab state the model needs after every op.
    fn state_json(&self) -> Value {
        let (url, title) = self.session.url_title();
        let mut state = json!({
            "tab": self.session.active_page(),
            "tabs": self.session.page_count(),
            "url": url,
            "title": title,
        });
        // The page is still being parsed (we stopped waiting for it): the
        // agent should `wait` for what it needs rather than trust `ax`.
        if self.session.is_loading() {
            state["loading"] = json!(true);
        }
        state
    }

    /// Tab inventory for `tab-list` / `tab-new` results.
    fn tabs_json(&self) -> Value {
        let active = self.session.active_page();
        let tabs: Vec<Value> = self
            .session
            .page_targets()
            .into_iter()
            .enumerate()
            .map(|(index, target)| json!({ "index": index, "target": target, "active": index == active }))
            .collect();
        json!({ "tabs": tabs, "active": active, "url": self.session.url() })
    }

    /// Run one op; returns the JSON `result` value.
    pub fn run(&mut self, command: &Command) -> anyhow::Result<Value> {
        self.run_op(command).map(|o| o.result)
    }

    /// Run one op, reporting dialogs answered and tabs opened along the way.
    pub fn run_op(&mut self, command: &Command) -> anyhow::Result<Outcome> {
        let mut new_tabs = self.session.sync_tabs();
        // A navigation an earlier op started (link click, JS redirect) must
        // finish before we read or act on the page — bounded by this op's
        // own timeout, and never fatal: a page that never finishes loading
        // must not wedge the session. Ops that navigate or don't read the
        // page skip it.
        let reads_page = !matches!(
            command,
            Command::Quit { .. }
                | Command::Dialog { .. }
                | Command::TabList {}
                | Command::TabNew { .. }
                | Command::TabSelect { .. }
                | Command::TabClose { .. }
                | Command::ClosePage { .. }
                | Command::Goto { .. }
                | Command::Open { .. }
                | Command::Reload {}
                | Command::Back {}
                | Command::Forward {}
                | Command::Wait { ms: Some(_), selector: None, text: None, url: None, gone: None, js: None, .. }
        );
        if reads_page {
            let budget = op_timeout_ms(command)
                .map(|ms| std::time::Duration::from_secs_f64(ms / 1000.0))
                .unwrap_or_else(|| self.session.nav_timeout());
            self.session.settle_within(budget);
            // Snapshots read what the page shows: give data it is still
            // fetching (an SPA's "Loading…" list) a moment to land, so the
            // agent doesn't spend a turn on a placeholder.
            if matches!(command, Command::Ax { .. } | Command::Screenshot { .. }) {
                self.session.network_quiet(NETWORK_QUIET, budget.min(NETWORK_QUIET_MAX));
            }
        }
        let result = self.dispatch(command).map_err(|e| {
            let msg = format!("{e:#}");
            if msg.contains("CDP connection closed") || msg.contains("CDP client is shut down") {
                let how = self.session.browser_exit().unwrap_or_else(|| "connection lost".into());
                anyhow::anyhow!("{msg} — the browser is gone ({how}); run `quit`, then `start` again")
            } else {
                e
            }
        });
        let opened = self.session.sync_tabs();
        if !opened.is_empty() {
            // The op opened a tab and the session switched to it: let its
            // page load before reporting it (bounded like any settle).
            let budget = op_timeout_ms(command)
                .map(|ms| std::time::Duration::from_secs_f64(ms / 1000.0))
                .unwrap_or_else(|| self.session.nav_timeout());
            self.session.settle_within(budget);
        }
        new_tabs.extend(opened);
        let dialogs = self.session.take_dialogs();
        let result = match (result, new_tabs.is_empty()) {
            // A click that opened a tab: report the state of the new tab.
            (Ok(r), false) if r.get("tab").is_some() => Ok(self.state_json()),
            (r, _) => r,
        }?;
        Ok(Outcome { result, dialogs, new_tabs })
    }

    fn dispatch(&mut self, command: &Command) -> anyhow::Result<Value> {
        let s = &self.session;
        match command {
            Command::Open { url } => {
                if let Some(url) = url {
                    self.goto(url, None, None)?;
                }
                Ok(self.state_json())
            }
            Command::Goto { url, wait, timeout_ms } => {
                self.goto(url, wait.as_deref(), *timeout_ms)?;
                Ok(self.state_json())
            }
            Command::Back {} => {
                s.back().map_err(|e| anyhow::anyhow!("back: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Forward {} => {
                s.forward().map_err(|e| anyhow::anyhow!("forward: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Reload {} => {
                s.reload().map_err(|e| anyhow::anyhow!("reload: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Click { target, timeout_ms } => {
                let t = target.target("click")?;
                let point =
                    s.click_target(&t, *timeout_ms).map_err(|e| anyhow::anyhow!("click {}: {e:#}", t.label()))?;
                let mut state = self.state_json();
                if point.get("synthetic").and_then(Value::as_bool) == Some(true) {
                    // The agent should know the click was not a real one.
                    state["synthetic_click"] = json!(if point.get("dropped").and_then(Value::as_bool) == Some(true) {
                        "the browser did not deliver the mouse events (twice); dispatched DOM events instead"
                            .to_string()
                    } else if point.get("covered").and_then(Value::as_bool) == Some(true) {
                        format!(
                            "element is covered by {}; dispatched DOM events instead (close the overlay if the click had no effect)",
                            point.get("covered_by").and_then(Value::as_str).unwrap_or("another element")
                        )
                    } else {
                        "element has no visible box; dispatched DOM events instead".to_string()
                    });
                }
                Ok(state)
            }
            Command::Hover { target, timeout_ms } => {
                let t = target.target("hover")?;
                let point = s.hover(&t, *timeout_ms).map_err(|e| anyhow::anyhow!("hover {}: {e:#}", t.label()))?;
                let mut state = self.state_json();
                if point.get("synthetic").and_then(Value::as_bool) == Some(true) {
                    state["synthetic_hover"] = json!(
                        "element is covered or has no box; sent DOM mouseover events only (CSS :hover will not apply)"
                    );
                }
                Ok(state)
            }
            Command::Fill { target, value, timeout_ms } => {
                let t = target.target("fill")?;
                s.fill_target(&t, value, *timeout_ms).map_err(|e| anyhow::anyhow!("fill {}: {e:#}", t.label()))?;
                Ok(self.state_json())
            }
            Command::Select { target, value, timeout_ms } => {
                let t = target.target("select")?;
                let chosen =
                    s.select(&t, value, *timeout_ms).map_err(|e| anyhow::anyhow!("select {}: {e:#}", t.label()))?;
                let mut state = self.state_json();
                state["selected"] = chosen;
                Ok(state)
            }
            Command::Press { key, target, timeout_ms } => {
                let t = target.optional("press")?;
                s.press(t.as_ref(), key, *timeout_ms).map_err(|e| anyhow::anyhow!("press {key}: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Type { value, target, timeout_ms } => {
                let t = target.optional("type")?;
                s.type_text(t.as_ref(), value, *timeout_ms).map_err(|e| anyhow::anyhow!("type: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Scroll { target, by, to, timeout_ms } => {
                let t = target.optional("scroll")?;
                s.scroll(t.as_ref(), *by, to.as_deref(), *timeout_ms).map_err(|e| anyhow::anyhow!("scroll: {e:#}"))
            }
            Command::Upload { target, files, timeout_ms } => {
                let t = target.target("upload")?;
                s.upload(&t, files, *timeout_ms).map_err(|e| anyhow::anyhow!("upload {}: {e:#}", t.label()))?;
                Ok(json!({ "files": files.len() }))
            }
            Command::Wait { selector, text, url, gone, js, ms, timeout_ms } => {
                if let Some(ms) = ms {
                    std::thread::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0));
                }
                let mut checks = Vec::new();
                let mut what = Vec::new();
                const VISIBLE: &str =
                    "(el) => !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden'";
                // Any match counts: a hidden template copy ahead of the
                // visible element must not decide the wait.
                if let Some(sel) = selector {
                    checks.push(format!("[...document.querySelectorAll({})].some({VISIBLE})", json!(sel)));
                    what.push(format!("selector {sel:?} visible"));
                }
                if let Some(sel) = gone {
                    checks.push(format!("![...document.querySelectorAll({})].some({VISIBLE})", json!(sel)));
                    what.push(format!("selector {sel:?} gone"));
                }
                if let Some(t) = text {
                    checks.push(format!("!!document.body && document.body.innerText.includes({})", json!(t)));
                    what.push(format!("text {t:?}"));
                }
                if let Some(u) = url {
                    checks.push(format!("location.href.includes({})", json!(u)));
                    what.push(format!("url contains {u:?}"));
                }
                if let Some(expr) = js {
                    checks.push(format!("!!(() => {{ try {{ const v = ({expr}\n); return typeof v === 'function' ? v() : v; }} catch (e) {{ return false; }} }})()"));
                    what.push(format!("js {expr:?}"));
                }
                if checks.is_empty() {
                    return Ok(json!({ "waited_ms": ms.unwrap_or(0.0) }));
                }
                let waited = s.wait_until(&checks.join(" && "), &what.join(" and "), *timeout_ms)?;
                let mut state = self.state_json();
                state["waited_ms"] = json!(waited);
                Ok(state)
            }
            Command::Dialog { accept, prompt_text } => {
                let accept = accept.unwrap_or(true);
                s.set_dialog_policy(accept, prompt_text.clone());
                Ok(json!({ "dialogs": if accept { "accept" } else { "dismiss" } }))
            }
            Command::Text { selector, timeout_ms } => {
                let text = s
                    .text_with_timeout(selector, *timeout_ms)
                    .map_err(|e| anyhow::anyhow!("text {selector:?}: {e:#}"))?;
                Ok(json!(text))
            }
            Command::Eval { expression, timeout_ms } => {
                let value: Value =
                    s.evaluate_with_timeout(expression, *timeout_ms).map_err(|e| anyhow::anyhow!("eval: {e:#}"))?;
                Ok(value)
            }
            Command::Title {} => {
                let title = s.title().map_err(|e| anyhow::anyhow!("title: {e:#}"))?;
                Ok(json!(title))
            }
            Command::Ax { max_depth, all, format, limit, refs, scope, timeout_ms } => {
                let json_format = match format.as_deref() {
                    // Serve-protocol clients that predate the text tree get
                    // the JSON list they always got.
                    None | Some("json") => true,
                    Some("text") => *all == Some(true),
                    Some(other) => anyhow::bail!("ax --format takes text|json, got {other:?}"),
                };
                if json_format && (scope.selector.is_some() || scope.node_ref.is_some() || scope.text.is_some()) {
                    anyhow::bail!("ax: --selector/ref scoping needs the text tree (drop --format json)");
                }
                let opts = AxOptions {
                    max_depth: *max_depth,
                    json: json_format,
                    all: all.unwrap_or(false),
                    all_refs: refs.as_deref() == Some("all"),
                    limit: limit.unwrap_or(DEFAULT_AX_LIMIT),
                    scope: scope.optional("ax")?,
                };
                let mut tree = s.ax(&opts, *timeout_ms).map_err(|e| anyhow::anyhow!("ax: {e:#}"))?;
                if let (Value::Array(nodes), Some(n)) = (&mut tree, limit) {
                    nodes.truncate(*n);
                }
                match tree {
                    Value::String(text) if self.session.page_count() > 1 => Ok(Value::String(
                        format!(
                            "{} (tab {} of {})",
                            text.lines().next().unwrap_or(""),
                            self.session.active_page(),
                            self.session.page_count()
                        ) + &text[text.find('\n').unwrap_or(text.len())..],
                    )),
                    other => Ok(other),
                }
            }
            Command::Url {} => Ok(self.state_json()),
            Command::TabList {} => Ok(self.tabs_json()),
            Command::TabNew { url } => {
                self.session.new_page().map_err(|e| anyhow::anyhow!("tab-new: {e:#}"))?;
                if let Some(url) = url {
                    self.goto(url, None, None)?;
                }
                Ok(self.tabs_json())
            }
            Command::TabSelect { index } => {
                self.session.select_page(*index).map_err(|e| anyhow::anyhow!("tab-select {index}: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::TabClose { index } | Command::ClosePage { index } => {
                let index = index.unwrap_or_else(|| self.session.active_page());
                self.session.close_page(index).map_err(|e| anyhow::anyhow!("tab-close {index}: {e:#}"))?;
                Ok(self.state_json())
            }
            Command::Screenshot { path, full_page } => {
                let path = path.clone().unwrap_or_else(|| {
                    format!(
                        "shot-{}.png",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    )
                });
                let bytes =
                    s.screenshot_png(full_page.unwrap_or(false)).map_err(|e| anyhow::anyhow!("screenshot: {e:#}"))?;
                std::fs::write(&path, &bytes).map_err(|e| anyhow::anyhow!("screenshot: writing {path}: {e:#}"))?;
                // Report where it really went (a relative path is relative
                // to this process, which may not be the caller's directory).
                let path = std::path::absolute(&path).map(|p| p.display().to_string()).unwrap_or(path);
                Ok(json!({ "path": path, "bytes": bytes.len() }))
            }
            // Attached: only the connection ends; the browser and every
            // window (navigera's included) stay open.
            Command::Quit { close_browser: true } if s.is_attached() => {
                s.close_browser()?;
                Ok(json!({ "bye": true, "browser": "closed" }))
            }
            Command::Quit { .. } if s.is_attached() => Ok(json!({ "bye": true, "browser": "left open" })),
            Command::Quit { .. } => Ok(json!({ "bye": true })),
        }
    }

    /// Navigate the active page, applying the engine-aware policy.
    fn goto(&self, url: &str, wait: Option<&str>, timeout_ms: Option<f64>) -> anyhow::Result<()> {
        let url = normalize_url(url);
        self.session
            .goto_wait(&url, wait.unwrap_or("load"), timeout_ms)
            .map_err(|e| anyhow::anyhow!("goto {url}: {e:#}"))
    }
}

/// `example.com` -> `https://example.com`; `localhost:3000` -> `http://…`.
/// Anything with a scheme (http:, file:, data:, about:) is left alone.
/// The op's own `timeout_ms`, if it carries one.
fn op_timeout_ms(command: &Command) -> Option<f64> {
    let v = serde_json::to_value(command).ok()?;
    v.get("timeout_ms").and_then(Value::as_f64)
}

pub fn normalize_url(url: &str) -> String {
    let u = url.trim();
    let has_scheme = u.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && !rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    });
    if has_scheme {
        return u.to_string();
    }
    let local = ["localhost", "127.", "0.0.0.0", "[::1]"].iter().any(|p| u.starts_with(p));
    format!("{}://{u}", if local { "http" } else { "https" })
}

/// Parse and run one protocol line against the live driver.
///
/// Returns `None` for blank lines, else the response plus whether the line
/// was `quit` (the caller owns shutdown). Shared by stdin `serve` and the
/// socket session server so both speak exactly the same protocol.
///
/// A line holding a JSON array is a batch: each command runs in order and
/// gets its own response line; a failure doesn't stop the rest, `quit` does.
/// (One line per batch sidesteps shells that don't turn `\n` into newlines.)
pub(crate) fn handle_line(driver: &mut Driver, raw: &str) -> Option<(Vec<Response>, bool)> {
    // PowerShell 5 pipes text to native programs with a UTF-8 BOM.
    let raw = raw.trim_start_matches('\u{feff}');
    if raw.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => {
            let mut responses = Vec::with_capacity(items.len());
            for item in items {
                let (response, is_quit) = handle_value(driver, item);
                responses.push(response);
                if is_quit {
                    return Some((responses, true));
                }
            }
            Some((responses, false))
        }
        Ok(value) => {
            let (response, is_quit) = handle_value(driver, value);
            Some((vec![response], is_quit))
        }
        Err(_) => {
            let mut error = "each line must be a JSON object (or an array of them)".to_string();
            if raw.contains("}\\n{") {
                error.push_str(
                    "; this line has a literal \\n between commands (PowerShell's echo doesn't \
                     turn \\n into a newline): send a JSON array on one line, or pipe a file",
                );
            }
            Some((vec![err_response(Value::Null, error, Instant::now())], false))
        }
    }
}

fn handle_value(driver: &mut Driver, value: Value) -> (Response, bool) {
    let (id, command) = match value {
        Value::Object(mut map) => {
            let id = map.remove("id").unwrap_or(Value::Null);
            match serde_json::from_value::<Command>(Value::Object(map)) {
                Ok(command) => (id, command),
                Err(e) => return (err_response(id, format!("bad command: {e}"), Instant::now()), false),
            }
        }
        _ => return (err_response(Value::Null, "each command must be a JSON object".into(), Instant::now()), false),
    };
    let started = Instant::now();
    let is_quit = matches!(command, Command::Quit { .. });
    let response = match driver.run_op(&command) {
        Ok(outcome) => outcome_response(id, outcome, started),
        Err(e) => {
            let mut r = err_response(id, format!("{e:#}"), started);
            let dialogs = driver.session().take_dialogs();
            if !dialogs.is_empty() {
                r.dialogs = Some(dialogs);
            }
            r
        }
    };
    (response, is_quit)
}

pub(crate) fn write_response(output: &mut dyn Write, response: &Response, pretty: bool) -> io::Result<()> {
    let line = if pretty { serde_json::to_string_pretty(response) } else { serde_json::to_string(response) }
        .expect("response serializes");
    writeln!(output, "{line}")?;
    output.flush()
}

/// Print a response for a human/agent CLI call: the JSON envelope, or with
/// `--raw` just the result (strings unquoted; notes and errors on stderr).
/// Returns whether the op succeeded.
pub fn print_cli_response(output: &mut dyn Write, response: &Value, config: &SessionConfig) -> bool {
    let ok = response.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !config.raw {
        let text = if config.pretty { serde_json::to_string_pretty(response) } else { serde_json::to_string(response) }
            .unwrap_or_else(|_| response.to_string());
        let _ = writeln!(output, "{text}");
        let _ = output.flush();
        return ok;
    }
    for d in response.get("dialogs").and_then(Value::as_array).into_iter().flatten() {
        eprintln!(
            "note: {} dialog {:?} was {}",
            d.get("type").and_then(Value::as_str).unwrap_or("dialog"),
            d.get("message").and_then(Value::as_str).unwrap_or(""),
            if d.get("accepted").and_then(Value::as_bool) == Some(true) { "accepted" } else { "dismissed" }
        );
    }
    if let Some(tabs) = response.get("new_tabs").and_then(Value::as_array) {
        eprintln!(
            "note: the page opened {} new tab(s); now on tab {}",
            tabs.len(),
            tabs.last().unwrap_or(&Value::Null)
        );
    }
    if ok {
        match response.get("result") {
            Some(Value::String(s)) => {
                let _ = writeln!(output, "{s}");
            }
            Some(other) => {
                let _ = writeln!(
                    output,
                    "{}",
                    if config.pretty {
                        serde_json::to_string_pretty(other).unwrap_or_default()
                    } else {
                        other.to_string()
                    }
                );
            }
            None => {}
        }
    } else {
        eprintln!("error: {}", response.get("error").and_then(Value::as_str).unwrap_or("unknown error"));
    }
    let _ = output.flush();
    ok
}

fn outcome_response(id: Value, outcome: Outcome, started: Instant) -> Response {
    let mut r = ok_response(id, outcome.result, started);
    if !outcome.dialogs.is_empty() {
        r.dialogs = Some(outcome.dialogs);
    }
    if !outcome.new_tabs.is_empty() {
        r.new_tabs = Some(outcome.new_tabs);
    }
    r
}

fn ok_response(id: Value, result: Value, started: Instant) -> Response {
    Response {
        id,
        ok: true,
        result: Some(result),
        error: None,
        dialogs: None,
        new_tabs: None,
        elapsed_ms: elapsed_ms(started),
    }
}

/// Elapsed milliseconds, rounded to the microsecond.
fn elapsed_ms(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn err_response(id: Value, error: String, started: Instant) -> Response {
    Response {
        id,
        ok: false,
        result: None,
        error: Some(error),
        dialogs: None,
        new_tabs: None,
        elapsed_ms: elapsed_ms(started),
    }
}

/// Peak RSS of this process in KB (VmHWM from /proc/self/status), if readable.
/// Used for the NAVIGERA_RSS_REPORT diagnostic.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Serve mode: launch one warm browser, then read JSON commands from `input`
/// and write one JSON response per line to `output`.
///
/// Returns the process exit code (`0` after `quit` or EOF, `1` on launch /
/// IO failure). Logs go to stderr so stdout stays protocol-clean.
pub fn serve(config: &SessionConfig, input: &mut dyn BufRead, output: &mut dyn Write) -> ExitCode {
    let mut driver = match Driver::launch(config) {
        Ok(driver) => driver,
        Err(e) => {
            eprintln!("navigera: {e:#}");
            return ExitCode::from(1);
        }
    };
    eprintln!(
        "navigera: live session on {} (engine={}); send JSON lines, {{\"op\":\"quit\"}} to exit",
        driver.session().endpoint(),
        config.engine,
    );

    for raw in input.lines() {
        let raw = match raw {
            Ok(line) => line,
            Err(e) => {
                eprintln!("navigera: stdin read failed: {e}");
                return ExitCode::from(1);
            }
        };
        let Some((responses, is_quit)) = handle_line(&mut driver, &raw) else {
            continue;
        };
        // Serve mode is a line protocol: one response == one line, always.
        // (`--pretty` would split a response across lines and desync any
        // line-reading client; it only applies to one-shot output.)
        for response in &responses {
            if let Err(e) = write_response(output, response, false) {
                eprintln!("[navigera] failed to write response: {e:#}");
                return ExitCode::from(1);
            }
        }
        if is_quit {
            driver.session().close();
            crate::timing::report();
            if std::env::var("NAVIGERA_RSS_REPORT").is_ok() {
                eprintln!("[navigera] peak RSS at quit: {:?} KB", peak_rss_kb());
            }
            return ExitCode::SUCCESS;
        }
    }
    // EOF: shut the session down cleanly.
    driver.session().close();
    crate::timing::report();
    eprintln!("navigera: stdin closed, session shut down");
    if std::env::var("NAVIGERA_RSS_REPORT").is_ok() {
        eprintln!("[navigera] peak RSS at EOF-shutdown: {:?} KB", peak_rss_kb());
    }
    ExitCode::SUCCESS
}

/// One-shot mode: launch the browser, run one command, close, write response.
pub fn oneshot(config: &SessionConfig, command: &Command, output: &mut dyn Write) -> ExitCode {
    let started = Instant::now();
    if config.attach.is_some() || config.profile.is_some() {
        // Each one-shot command would open (and abandon) a window in the
        // user's browser, and Chrome would ask to Allow every one of them.
        let flag = if config.attach.is_some() { "--attach" } else { "--profile" };
        let error = format!(
            "{flag} keeps one connection to a browser that stays open: use a session, \
             e.g. `navigera -s me start {flag} {}` then `navigera -s me <command>`",
            config.attach.as_deref().or(config.profile.as_deref()).unwrap_or("")
        );
        print_cli_response(output, &json!({ "id": null, "ok": false, "error": error }), config);
        return ExitCode::from(2);
    }
    let mut driver = match Driver::launch(config) {
        Ok(driver) => driver,
        Err(e) => {
            eprintln!("navigera: {e:#}");
            return ExitCode::from(1);
        }
    };
    let response = match driver.run_op(command) {
        Ok(outcome) => outcome_response(Value::Null, outcome, started),
        Err(e) => err_response(Value::Null, format!("{e:#}"), started),
    };
    driver.session().close();
    crate::timing::report();
    let value = serde_json::to_value(&response).unwrap_or(Value::Null);
    if print_cli_response(output, &value, config) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Run a browser-less command (`skill`, `install-skill`, `--version`).
pub fn run_local(local: &Local, config: &SessionConfig, output: &mut dyn Write) -> ExitCode {
    match local {
        Local::Version => {
            let _ = writeln!(output, "navigera {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Local::Skill => {
            let _ = write!(output, "{SKILL_MD}");
            ExitCode::SUCCESS
        }
        Local::InstallSkill { dir } => {
            let response = match install_skill(dir) {
                Ok(path) => json!({ "id": null, "ok": true, "result": { "installed": path } }),
                Err(e) => json!({ "id": null, "ok": false, "error": format!("install-skill: {e:#}") }),
            };
            if print_cli_response(output, &response, config) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn cmd(items: &[&str]) -> Command {
        let mut all = vec!["navigera"];
        all.extend_from_slice(items);
        parse_args(argv(&all)).expect("valid invocation").command.expect("a command")
    }

    #[test]
    fn parses_one_shot_eval_with_trailing_global_flag() {
        let parsed = parse_args(argv(&["navigera", "eval", "--expression", "() => 40 + 2", "--pretty"]))
            .expect("valid invocation");
        assert!(parsed.config.pretty, "globals may trail the command");
        assert_eq!(parsed.config.engine, "chrome");
        assert_eq!(parsed.command, Some(Command::Eval { expression: "() => 40 + 2".into(), timeout_ms: None }));
    }

    #[test]
    fn parses_globals_before_command() {
        let parsed = parse_args(argv(&[
            "navigera",
            "--headed",
            "--engine",
            "chrome",
            "--chromium",
            "C:/chrome.exe",
            "goto",
            "--url",
            "https://example.com",
            "--wait",
            "domcontentloaded",
        ]))
        .expect("valid invocation");
        assert!(parsed.config.headed);
        assert_eq!(parsed.config.engine, "chrome");
        assert_eq!(parsed.config.chromium.as_deref(), Some("C:/chrome.exe"));
        assert_eq!(
            parsed.command,
            Some(Command::Goto {
                url: "https://example.com".into(),
                wait: Some("domcontentloaded".into()),
                timeout_ms: None,
            })
        );
    }

    #[test]
    fn positional_arguments_like_other_agent_clis() {
        assert_eq!(
            cmd(&["goto", "example.com"]),
            Command::Goto { url: "example.com".into(), wait: None, timeout_ms: None }
        );
        assert_eq!(
            cmd(&["click", "@e12"]),
            Command::Click { target: TargetArgs { node_ref: Some(12), ..Default::default() }, timeout_ms: None }
        );
        assert_eq!(
            cmd(&["click", "#buy", "--timeout-ms", "900"]),
            Command::Click {
                target: TargetArgs { selector: Some("#buy".into()), ..Default::default() },
                timeout_ms: Some(900.0)
            }
        );
        assert_eq!(
            cmd(&["fill", "31", "hello world"]),
            Command::Fill {
                target: TargetArgs { node_ref: Some(31), ..Default::default() },
                value: "hello world".into(),
                timeout_ms: None
            }
        );
        assert_eq!(
            cmd(&["click", "--text", "Add to cart"]),
            Command::Click {
                target: TargetArgs { text: Some("Add to cart".into()), ..Default::default() },
                timeout_ms: None
            }
        );
        assert_eq!(
            cmd(&["press", "Enter"]),
            Command::Press { key: "Enter".into(), target: TargetArgs::default(), timeout_ms: None }
        );
        assert_eq!(
            cmd(&["select", "7", "Canada"]),
            Command::Select {
                target: TargetArgs { node_ref: Some(7), ..Default::default() },
                value: vec!["Canada".into()],
                timeout_ms: None
            }
        );
        assert_eq!(
            cmd(&["eval", "document.title"]),
            Command::Eval { expression: "document.title".into(), timeout_ms: None }
        );
        assert_eq!(
            cmd(&["wait", "--text", "Order placed"]),
            Command::Wait {
                selector: None,
                text: Some("Order placed".into()),
                url: None,
                gone: None,
                js: None,
                ms: None,
                timeout_ms: None
            }
        );
        assert_eq!(
            cmd(&["wait", "250"]),
            Command::Wait {
                selector: None,
                text: None,
                url: None,
                gone: None,
                js: None,
                ms: Some(250.0),
                timeout_ms: None
            }
        );
        assert_eq!(cmd(&["tab-select", "1"]), Command::TabSelect { index: 1 });
        assert_eq!(cmd(&["snapshot"]), cmd(&["ax"]));
        assert_eq!(cmd(&["close"]), Command::Quit { close_browser: false });
        assert_eq!(cmd(&["quit", "--close-browser"]), Command::Quit { close_browser: true });
    }

    #[test]
    fn urls_get_a_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("localhost:3000/x"), "http://localhost:3000/x");
        assert_eq!(normalize_url("127.0.0.1:8000"), "http://127.0.0.1:8000");
        assert_eq!(normalize_url("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(normalize_url("file:///tmp/a.html"), "file:///tmp/a.html");
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }

    #[test]
    fn serve_mode_has_no_command() {
        let parsed = parse_args(argv(&["navigera", "serve", "--timeout-ms", "5000"])).expect("valid invocation");
        assert!(parsed.command.is_none());
        assert_eq!(parsed.config.timeout_ms, 5000.0);
    }

    #[test]
    fn help_and_invalid_invocations_are_distinguished() {
        let help = parse_args(argv(&["navigera", "--help"])).expect_err("help");
        assert!(matches!(help, ArgsError::Help(_)), "explicit help is Help, not Invalid: {help:?}");
        let no_args = parse_args(argv(&["navigera"])).expect_err("no args");
        assert!(matches!(no_args, ArgsError::Help(_)));
        match parse_args(argv(&["navigera", "help", "click"])).expect_err("op help") {
            ArgsError::Help(text) => assert!(text.contains("usage:") && text.contains("click"), "{text}"),
            other => panic!("{other:?}"),
        }
        match parse_args(argv(&["navigera", "fill", "--help"])).expect_err("op help") {
            ArgsError::Help(text) => assert!(text.contains("fill"), "{text}"),
            other => panic!("{other:?}"),
        }

        let unknown = parse_args(argv(&["navigera", "navigat"])).expect_err("unknown");
        match unknown {
            ArgsError::Invalid(message) => {
                assert!(message.contains("unknown command"), "{message}");
                assert!(message.contains("did you mean `goto`") || message.contains("commands:"), "{message}");
            }
            other => panic!("unknown command must be Invalid: {other:?}"),
        }
        match parse_args(argv(&["navigera", "evaluat"])).expect_err("typo") {
            ArgsError::Invalid(message) => assert!(message.contains("did you mean `eval`"), "{message}"),
            other => panic!("{other:?}"),
        }

        let missing = parse_args(argv(&["navigera", "goto"])).expect_err("missing url");
        assert!(matches!(missing, ArgsError::Invalid(_)));
        let extra = parse_args(argv(&["navigera", "title", "x"])).expect_err("extra");
        assert!(matches!(extra, ArgsError::Invalid(m) if m.contains("unexpected argument")));
    }

    #[test]
    fn tab_commands_parse_indexes() {
        assert_eq!(cmd(&["tab-select", "--index", "2"]), Command::TabSelect { index: 2 });
        assert_eq!(cmd(&["tab-close"]), Command::TabClose { index: None });
        assert!(parse_args(argv(&["navigera", "tab-select"])).is_err());
        assert!(parse_args(argv(&["navigera", "tab-select", "--index", "x"])).is_err());
    }

    #[test]
    fn command_deserializes_from_protocol_json() {
        let command: Command =
            serde_json::from_str(r##"{"op":"fill","selector":"#kw","value":"600-10070","timeout_ms":10000}"##)
                .expect("valid command");
        assert_eq!(
            command,
            Command::Fill {
                target: TargetArgs { selector: Some("#kw".into()), ..Default::default() },
                value: "600-10070".into(),
                timeout_ms: Some(10000.0),
            }
        );
        let command: Command = serde_json::from_str(r#"{"op":"click","ref":"@e7"}"#).expect("string ref");
        assert_eq!(
            command,
            Command::Click { target: TargetArgs { node_ref: Some(7), ..Default::default() }, timeout_ms: None }
        );
        let command: Command = serde_json::from_str(r#"{"op":"select","ref":3,"values":["a","b"]}"#).expect("values");
        assert!(
            matches!(command, Command::Select { ref value, .. } if value == &vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn click_targets_are_exclusive() {
        let both =
            parse_args(argv(&["navigera", "click", "--ref", "1", "--selector", "a"])).expect("parses").command.unwrap();
        let mut driver_free = match both {
            Command::Click { target, .. } => target,
            _ => unreachable!(),
        };
        assert!(driver_free.target("click").is_err(), "selector and ref are mutually exclusive");
        driver_free = TargetArgs::default();
        assert!(driver_free.target("click").is_err(), "a target is required");
        assert!(parse_args(argv(&["navigera", "fill", "--value", "x"])).is_ok_and(|p| matches!(
            p.command,
            Some(Command::Fill { ref target, .. }) if *target == TargetArgs::default()
        )));
    }

    #[test]
    fn commands_round_trip_through_json_for_session_clients() {
        for command in [
            Command::Goto { url: "https://example.com".into(), wait: None, timeout_ms: None },
            Command::Click { target: TargetArgs { node_ref: Some(9), ..Default::default() }, timeout_ms: Some(500.0) },
            Command::Ax {
                max_depth: None,
                all: Some(true),
                format: None,
                limit: Some(10),
                refs: None,
                scope: TargetArgs { selector: Some("main".into()), ..Default::default() },
                timeout_ms: None,
            },
            Command::Select {
                target: TargetArgs { text: Some("Country".into()), ..Default::default() },
                value: vec!["CA".into()],
                timeout_ms: None,
            },
            Command::Wait {
                selector: None,
                text: Some("Done".into()),
                url: None,
                gone: None,
                js: None,
                ms: None,
                timeout_ms: None,
            },
            Command::Quit { close_browser: false },
            Command::Quit { close_browser: true },
        ] {
            let wire = serde_json::to_string(&command).expect("serializes");
            let back: Command = serde_json::from_str(&wire).expect("deserializes");
            assert_eq!(back, command, "{wire}");
        }
        let wire = serde_json::to_string(&Command::Click {
            target: TargetArgs { node_ref: Some(9), ..Default::default() },
            timeout_ms: None,
        })
        .unwrap();
        assert_eq!(wire, r#"{"op":"click","ref":9}"#);
    }

    #[test]
    fn session_flags_and_start() {
        let parsed = parse_args(argv(&["navigera", "--session", "work", "--idle-timeout-s", "60", "start"]))
            .expect("valid invocation");
        assert!(parsed.start);
        assert!(parsed.command.is_none());
        assert_eq!(parsed.config.session.as_deref(), Some("work"));
        assert_eq!(parsed.config.idle_timeout_s, 60);

        let parsed =
            parse_args(argv(&["navigera", "title", "--session", "work", "--raw"])).expect("trailing --session");
        assert_eq!(parsed.config.session.as_deref(), Some("work"));
        assert!(parsed.config.raw);
        assert!(!parsed.start);
        assert_eq!(parse_args(argv(&["navigera", "-s=w", "title"])).unwrap().config.session.as_deref(), Some("w"));
        assert_eq!(parse_args(argv(&["navigera", "title", "-s", "w"])).unwrap().config.session.as_deref(), Some("w"));
        let parsed = parse_args(argv(&["navigera", "--session=work", "install-skill", "--claude"])).expect("local");
        assert_eq!(parsed.local, Some(Local::InstallSkill { dir: ".claude/skills".into() }));
    }

    #[test]
    fn attach_and_profile_flags() {
        let attach = |args: &[&str]| parse_args(argv(args)).map(|p| (p.config.attach, p.start));
        // Bare --attach means the user's Chrome, wherever it sits.
        assert_eq!(attach(&["navigera", "-s", "me", "start", "--attach"]).unwrap(), (Some("chrome".into()), true));
        assert_eq!(attach(&["navigera", "-s", "me", "--attach", "start"]).unwrap(), (Some("chrome".into()), true));
        assert_eq!(
            attach(&["navigera", "-s", "me", "start", "--attach", "--headless"]).unwrap().0.as_deref(),
            Some("chrome")
        );
        assert_eq!(attach(&["navigera", "-s", "me", "start", "--attach", "edge"]).unwrap().0.as_deref(), Some("edge"));
        assert_eq!(attach(&["navigera", "-s", "me", "--attach=9222", "start"]).unwrap().0.as_deref(), Some("9222"));
        assert_eq!(
            attach(&["navigera", "--attach", "/tmp/ud", "-s", "me", "start"]).unwrap().0.as_deref(),
            Some("/tmp/ud")
        );
        let p = parse_args(argv(&["navigera", "-s", "me", "start", "--profile", "work", "--headless"])).unwrap();
        assert_eq!(p.config.profile.as_deref(), Some("work"));
        assert!(p.config.headless);
        assert!(parse_args(argv(&["navigera", "-s", "me", "start", "--attach", "--profile", "w"])).is_err());
        assert!(parse_args(argv(&["navigera", "--engine", "lightpanda", "-s", "me", "start", "--attach"])).is_err());
    }

    #[test]
    fn response_serializes_protocol_shape() {
        let ok = ok_response(serde_json::json!(7), serde_json::json!({"url": "https://example.com/"}), Instant::now());
        let line = serde_json::to_string(&ok).expect("serializes");
        assert!(line.contains("\"id\":7"));
        assert!(line.contains("\"ok\":true"));
        assert!(!line.contains("error") && !line.contains("dialogs"), "successful responses omit `error`: {line}");

        let err = err_response(Value::Null, "boom".into(), Instant::now());
        let line = serde_json::to_string(&err).expect("serializes");
        assert!(line.contains("\"ok\":false") && line.contains("\"error\":\"boom\""));
    }

    #[test]
    fn refs_parse_in_every_spelling() {
        for raw in ["12", "@12", "e12", "ref=12", "[ref=12]", " 12 "] {
            assert_eq!(parse_ref(raw), Some(12), "{raw}");
        }
        assert_eq!(parse_ref("#buy"), None);
        assert_eq!(parse_ref("button"), None);
    }
}
