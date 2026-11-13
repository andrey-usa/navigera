//! Interactions that practice sites showed going wrong, each against an
//! offline page that behaves the same way (`tests/fixtures/interactions_site.py`):
//! a click on a covered element, elements inside frames and shadow roots,
//! multiple and non-breaking-space selects, a slow page, downloads, a modal
//! that wires its buttons once it has opened, an input under a panel, and
//! right clicks.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

fn tool_exe() -> String {
    env!("CARGO_BIN_EXE_navigera").to_string()
}

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true);
    assert!(
        found.is_some() || std::env::var_os("NAVIGERA_REQUIRE_BROWSER").is_none(),
        "NAVIGERA_REQUIRE_BROWSER is set but no Chrome/Chromium was found"
    );
    found.is_some()
}

struct Site {
    child: Child,
    url: String,
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn site() -> Option<Site> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/interactions_site.py"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site { child, url })
}

/// A session with its own browser, quit however the test ends.
struct Session {
    name: String,
    _downloads: Option<tempfile::TempDir>,
}

impl Session {
    fn start(test: &str, extra: &[&str]) -> Session {
        let name = std::env::temp_dir()
            .join(format!("nv-interactions-{test}-{}.sock", std::process::id()))
            .display()
            .to_string();
        let downloads = tempfile::tempdir().unwrap();
        let dir = downloads.path().display().to_string();
        let mut args = vec!["start", "--download-dir", &dir];
        args.extend_from_slice(extra);
        let s = Session { name, _downloads: Some(downloads) };
        s.ok(&args);
        s
    }

    fn run(&self, args: &[&str]) -> (bool, Value) {
        let out = Command::new(tool_exe())
            .arg("--session")
            .arg(&self.name)
            .args(args)
            // Clicks say what they saw change (`effect_trace`): failures show it.
            .env("NAVIGERA_TRACE_EFFECT", "1")
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        let text = String::from_utf8_lossy(&out.stdout);
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| {
            panic!(
                "stdout must be one JSON object for {args:?}: {text}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.success(), value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (success, r) = self.run(args);
        assert!(success && r["ok"] == true, "{args:?} failed: {r}");
        r["result"].clone()
    }

    fn fails(&self, args: &[&str]) -> String {
        let (success, r) = self.run(args);
        assert!(!success && r["ok"] == false, "{args:?} should fail: {r}");
        r["error"].as_str().unwrap_or_default().to_string()
    }

    fn text(&self, args: &[&str]) -> String {
        self.ok(args).as_str().map(str::to_string).unwrap_or_else(|| self.ok(args).to_string())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new(tool_exe()).arg("--session").arg(&self.name).arg("quit").output();
    }
}

/// Something that takes the element's place between the hit test and the
/// press (here an overlay the mouse move opens) gets no press: the page
/// never sees it, and the click is pressed again once the element is
/// there. Before, the press clicked the overlay's own button, and the
/// report said what that changed, as if it were the click's.
#[test]
fn a_press_that_would_land_on_something_else_is_held_back_and_sent_again() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("hover-cover", &[]);
    for _ in 0..3 {
        s.ok(&["goto", &format!("{}/hover-cover", site.url)]);
        s.ok(&["click", "--selector", "#go"]);
        assert_eq!(s.text(&["text", "--selector", "#out"]), "clicked 1");
    }
}

/// A page that answers the press by moving the element (a banner put in
/// above it on pointerdown) has the click event go to the parent the press
/// and the release share; one that replaces it on mousedown gets that too,
/// or no click event at all (Chrome 153, 157). Either way the element's own
/// handler never runs. The page saw the press, so it isn't pressed again,
/// but the result says so. Before, it read as a plain success.
#[test]
fn a_click_whose_click_event_lands_off_the_element_says_so() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("click-off", &[]);
    for q in ["replace", "banner"] {
        s.ok(&["goto", &format!("{}/click-off?{q}", site.url)]);
        let r = s.ok(&["click", "--selector", "#go"]);
        let said = r["click_missed"].as_str().unwrap_or_default();
        let told = said.contains("went to div#w") || (q == "replace" && said.contains("sent no click event"));
        assert!(told && said.contains("handler didn't run"), "{q}: {r}");
        assert_eq!(s.text(&["text", "--selector", "#out"]), "idle", "{q}: pressed again");
    }
    s.ok(&["goto", &format!("{}/click-off", site.url)]);
    let r = s.ok(&["click", "--selector", "#go"]);
    assert!(r.get("click_missed").is_none(), "a still button: {r}");
    assert_eq!(s.text(&["text", "--selector", "#out"]), "went");
}

/// A locator by role and name, or by label, costs one pass over the page:
/// each candidate's `el.labels` used to scan the document on its first read,
/// so the first call on 12000 buttons took about 9 s. Timed at two sizes so a
/// slow runner can't fail it: linear is about 4x for 4x the buttons,
/// quadratic about 16x. Each size is built three times and its fastest run
/// counts: the other tests' browsers share the runner's cores, and a stall
/// during the large page alone read as 8.8x on macOS.
#[test]
fn a_locator_on_a_long_page_is_linear() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("long-page", &[]);
    let timed = |n: usize| {
        s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
        s.ok(&["eval", &format!("(() => {{ let h = '<label>Last <input id=last></label><ul>'; for (let i = 0; i < {n}; i++) h += '<li>Item ' + i + ' <button>Add ' + i + '</button></li>'; document.body.insertAdjacentHTML('beforeend', h + '</ul>'); return 1; }})()")]);
        let started = std::time::Instant::now();
        assert_eq!(
            s.text(&["get", "text", "--role", "button", "--name", &format!("Add {}", n - 1)]),
            format!("Add {}", n - 1)
        );
        s.ok(&["fill", "--label", "Last", "x"]);
        assert_eq!(s.text(&["get", "value", "--selector", "#last"]), "x");
        started.elapsed()
    };
    let best = |n: usize| (0..3).map(|_| timed(n)).min().unwrap();
    let (small, large) = (best(3000), best(12000));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(0.001);
    // The ratio alone swings with the small page's time (55-200 ms on one
    // runner); quadratic took about 9 s here, so a large page under 2 s
    // passes whatever the ratio.
    assert!(
        ratio < 8.0 || large < std::time::Duration::from_secs(2),
        "3000 buttons {small:?}, 12000 {large:?}: {ratio:.1}x"
    );
    assert!(large < std::time::Duration::from_secs(30), "{large:?}");
}

/// A wait sees a change within a frame of it, not at its next poll. The
/// page notes when the element was added and when the wait's own check
/// first saw it: times in the page, so the test's commands don't count.
/// Checking every 100 ms put that at 50 ms on average.
#[test]
fn a_wait_sees_a_change_within_a_frame() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("wake", &[]);
    s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
    let mut after = Vec::new();
    for i in 0..7 {
        let delay = 120 + 37 * i;
        s.ok(&[
            "eval",
            &format!(
                "window.__seen = 0; document.querySelectorAll('.late').forEach((e) => e.remove()); \
             setTimeout(() => {{ const p = document.createElement('p'); p.className = 'late'; \
             p.textContent = 'late {i}'; document.body.append(p); window.__added = performance.now(); }}, {delay}); 1"
            ),
        ]);
        s.ok(&[
            "wait",
            "--js",
            "document.querySelector('.late') && (window.__seen = window.__seen || performance.now())",
        ]);
        after.push(s.ok(&["eval", "window.__seen - window.__added"]).as_f64().unwrap());
    }
    after.sort_by(f64::total_cmp);
    assert!(after[3] < 35.0, "seen this many ms after the change: {after:?}");
}

/// A wait inside a frame sees a change there within a frame of it, not at
/// its next poll: an element added to the frame 120-400 ms after the wait
/// starts. The time from its adding to the next command's clock reading
/// (`Date.now`, one clock for both documents) holds the wait's return and
/// that command's start-up too, which is most of it on a slow or busy
/// machine (Windows CI, 16 ms a command). So each sample is paired with a
/// control on the same frame: the stamp is made by an `eval`, then the same
/// wait, already satisfied, returns at once through the same return path,
/// then the same reading. The control holds the commands' time and slightly
/// more (one more start-up), so the wake is at most what the difference
/// shows: the median of the per-sample differences. Looking every 100 ms put
/// it near 50 ms.
#[test]
fn a_wait_in_a_frame_sees_a_change_within_a_frame() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("framewake", &[]);
    s.ok(&["goto", &format!("{}/iframe-form", site.url)]);
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    for (what, flag) in [("selector", "--selector"), ("text", "--text")] {
        let (mut seen, mut diff) = (Vec::new(), Vec::new());
        for i in 0..15 {
            let delay = 120 + 20 * i;
            let (selector, text) = (".late", format!("late {what} {i}"));
            s.ok(&[
                "eval",
                &format!(
                    "const d = document.getElementById('email-subscribe').contentDocument; \
                     d.querySelectorAll('.late').forEach((e) => e.remove()); \
                     setTimeout(() => {{ const p = d.createElement('p'); p.className = 'late'; \
                     p.textContent = '{text}'; d.body.append(p); window.__added = Date.now(); }}, {delay}); 1"
                ),
            ]);
            let target = if flag == "--selector" { selector.to_string() } else { text };
            let wait = ["wait", flag, &target, "--frame", "#email-subscribe"];
            s.ok(&wait);
            let after = s.ok(&["eval", "Date.now() - window.__added"]).as_f64().unwrap();
            // The control: the element is there now, so the same wait returns at once.
            s.ok(&["eval", "window.__added = Date.now(); 1"]);
            s.ok(&wait);
            let own = s.ok(&["eval", "Date.now() - window.__added"]).as_f64().unwrap();
            seen.push(after);
            diff.push(after - own);
        }
        let (seen, diff) = (median(&mut seen), median(&mut diff));
        eprintln!("wait {flag} --frame: seen {seen} ms after adding, {diff} ms over the same commands with no wait");
        assert!(
            diff < 25.0,
            "wait {flag} --frame saw it {seen} ms after it was added, {diff} ms over its own commands (median of 15)"
        );
    }
}

/// A headless Chrome has no pointing device and says so (`hover: none`,
/// `pointer: none`), which leaves an app that gates its mouse handlers on
/// hover inert. navigera's launch says there is a mouse, as a headed Chrome
/// on a desktop does; a touch device still reports a coarse pointer and no
/// hover, and the next page after
/// `set device off` is back to the mouse.
#[test]
fn headless_reports_a_mouse_and_a_touch_device_reports_touch() {
    if !chrome() {
        return;
    }
    let s = Session::start("pointer", &[]);
    s.ok(&["goto", "about:blank"]);
    let q = |query: &str| s.ok(&["eval", &format!("matchMedia('{query}').matches")]).as_bool().unwrap();
    let mouse = || q("(hover: hover)") && q("(any-hover: hover)") && q("(pointer: fine)") && q("(any-pointer: fine)");
    assert!(mouse(), "headless: no mouse");
    s.ok(&["set", "device", "iPhone 15"]);
    assert!(q("(pointer: coarse)") && q("(hover: none)") && !q("(hover: hover)"), "iPhone: not touch");
    s.ok(&["set", "device", "off"]);
    s.ok(&["goto", "about:blank"]);
    assert!(mouse(), "after set device off: no mouse");
    // Turning touch emulation off in headless leaves the current page with no
    // pointer until the next one, as after `set device off`.
    s.ok(&["set", "device", "Desktop"]);
    s.ok(&["goto", "about:blank"]);
    assert!(mouse(), "Desktop: no mouse");
}

/// A frame of another origin is out of the observer's reach: a wait there
/// still sees what appears in it (looking again every 100 ms), on the same
/// site (another port) and across sites (its own process).
#[test]
fn a_wait_in_a_cross_origin_frame_still_sees_a_change() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("framewake-xo", &[]);
    for page in ["/xo-parent", "/xo-parent?same-site"] {
        s.ok(&["goto", &format!("{}{page}", site.url)]);
        // The frame loads a page of its own origin whose button appears 300 ms later.
        s.ok(&["eval", "const f = document.getElementById('card'); f.src = new URL(f.src).origin + '/slow?ms=300'; 1"]);
        let waited = s.ok(&["wait", "--text", "Late button", "--frame", "#card", "--timeout-ms", "5000"]);
        assert!(waited["waited_ms"].as_f64().unwrap() < 1500.0, "{page}: {waited}");
    }
}

/// `eval` takes what people type: an expression, an arrow function,
/// statements with the last value the answer, top-level `await`, or a body
/// that `return`s.
#[test]
fn text_grep_prints_the_passages_that_mention_the_words() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("grep", &[]);
    s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
    // A long page with one fact in it.
    s.ok(&[
        "eval",
        "document.body.innerHTML = '<h1>Returns</h1>' + Array.from({length: 300}, (_, i) => \
         `<p>Paragraph ${i} of filler.</p>`).join('') + '<h2>Fees</h2><p>The restocking fee is <b>8%</b> \
         after 37 days.</p><p>Ask support.</p>'; 1",
    ]);
    let whole = s.text(&["text"]);
    let out = s.text(&["text", "--grep", "Restocking Fee"]);
    assert!(out.starts_with("1 passage with \"Restocking Fee\":\n"), "{out}");
    // The heading before it and the line after it come along, as they read.
    assert!(out.contains(": Fees\n"), "{out}");
    assert!(out.contains(": The restocking fee is 8% after 37 days.\n"), "{out}");
    assert!(out.ends_with(": Ask support."), "{out}");
    assert!(out.len() * 20 < whole.len(), "{} of {} bytes", out.len(), whole.len());
    // Many matches: the first few, and how many there are.
    let out = s.text(&["text", "--grep", "filler", "--context", "0", "--limit", "2"]);
    assert!(out.starts_with("2 of 300 passages with \"filler\" (--limit 300 for all"), "{out}");
    // The tree, pruned the same way: a ref to act on without reading the page.
    s.ok(&[
        "eval",
        "document.body.insertAdjacentHTML('beforeend', '<nav aria-label=\"Help\"><a href=\"#fees\">Fee \
           schedule</a></nav>'); 1",
    ]);
    let out = s.text(&["ax", "--grep", "fee schedule"]);
    assert!(out.contains("1 line with \"fee schedule\", under their containers:\n- navigation \"Help\":\n"), "{out}");
    let link = out.lines().last().unwrap();
    assert!(link.trim_start().starts_with("- link \"Fee schedule\" [ref="), "{out}");
    let number = link.split("[ref=").nth(1).unwrap().split(']').next().unwrap();
    s.ok(&["click", number]);
    assert!(s.ok(&["eval", "location.hash"]) == "#fees");
    // Inside one element, and nothing found is an answer, not a failure.
    let out = s.text(&["text", "--selector", "h2", "--grep", "fees"]);
    assert!(out.ends_with("1: Fees"), "{out}");
    assert!(s.text(&["text", "--grep", "warranty"]).starts_with("no line of the text has \"warranty\""));
}

#[test]
fn eval_takes_statements_await_and_a_body() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("eval", &[]);
    s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
    assert_eq!(s.ok(&["eval", "document.title"]), "hidden layers");
    assert_eq!(s.ok(&["eval", "() => 40 + 2"]), 42);
    assert_eq!(s.ok(&["eval", "const n = 40; n + 2"]), 42);
    assert_eq!(s.ok(&["eval", "const r = await fetch(location.href); r.status"]), 200);
    assert_eq!(s.ok(&["eval", "const n = 6 * 7; return n"]), 42);
    assert!(s.fails(&["eval", "((("]).contains("SyntaxError"));
    // Text where CSS goes is said at once, with what to write instead.
    let started = std::time::Instant::now();
    let m = s.fails(&["click", "--selector", "35.75"]);
    assert!(m.contains("\"35.75\" is not a CSS selector (--text \"35.75\""), "{m}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
}

#[test]
fn a_covered_element_is_not_clicked_unless_forced() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("covered", &[]);
    s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
    s.ok(&["click", "--selector", "#greenButton"]);
    let err = s.fails(&["click", "--selector", "#greenButton", "--timeout-ms", "600"]);
    assert!(err.contains("covered by button#blueButton"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert_eq!(s.ok(&["get", "count", ".bg-warning"]), 0, "nothing reached the covered button");
    // --force: DOM events on the element itself, and the page can tell.
    let r = s.ok(&["click", "--selector", "#greenButton", "--force"]);
    assert!(r.get("synthetic_click").is_some(), "{r}");
    assert_eq!(s.ok(&["get", "count", ".bg-warning"]), 1);
}

#[test]
fn locators_reach_into_same_origin_frames() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("frames", &[]);
    s.ok(&["goto", &format!("{}/iframe-form", site.url)]);
    s.ok(&["fill", "--selector", "#email", "ada@example.com"]);
    s.ok(&["click", "--role", "button", "--name", "Subscribe"]);
    s.ok(&["wait", "--text", "Thanks ada@example.com"]);
    assert_eq!(s.text(&["get", "value", "--label", "Email"]), "ada@example.com");

    s.ok(&["goto", &format!("{}/frameset", site.url)]);
    s.ok(&["wait", "--text", "MIDDLE"]);
    assert_eq!(s.text(&["get", "text", "--text", "MIDDLE"]), "MIDDLE");
    assert_eq!(s.text(&["text", "--selector", "#content"]), "LEFT");
    assert_eq!(s.ok(&["get", "count", "#content"]), 4, "every frame's #content");

    s.ok(&["goto", &format!("{}/srcdoc", site.url)]);
    s.ok(&["click", "--text", "Deep button"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "deep clicked");
    // Its box is where it is on the page, both frames' offsets added.
    let b = s.ok(&["get", "box", "--text", "Deep button"]);
    let want = s.ok(&[
        "eval",
        "(() => { let x = 0, y = 0, d = document; for (const id of ['host', 'innerframe']) { \
         const f = id === 'host' ? d.querySelector('#host iframe') : d.getElementById(id); const r = f.getBoundingClientRect(); \
         x += r.left + f.clientLeft; y += r.top + f.clientTop; d = f.contentDocument; } \
         const r = d.getElementById('deep').getBoundingClientRect(); return [x + r.x, y + r.y]; })()",
    ]);
    for (k, i) in [("x", 0), ("y", 1)] {
        let (got, want) = (b[k].as_f64().unwrap(), want[i].as_f64().unwrap());
        assert!((got - want).abs() < 0.5 && want > 20.0, "{k}: {got} vs {want} ({b})");
    }
}

#[test]
fn css_selectors_reach_into_open_shadow_roots() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("shadow", &[]);
    s.ok(&["goto", &format!("{}/shadow-css", site.url)]);
    s.ok(&["click", "--selector", "#buttonGenerate"]);
    assert!(s.text(&["get", "value", "--selector", "#editField"]).starts_with("guid-"));
    s.ok(&["click", "--selector", "#nested"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#nested-out"]), "nested clicked");
    s.ok(&["wait", "--selector", "#nested"]);
}

#[test]
fn selects_add_to_a_multiple_and_ignore_nbsp() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("select", &[]);
    s.ok(&["goto", &format!("{}/multi-select", site.url)]);
    s.ok(&["select", "--selector", "#cars", "volvo"]);
    let r = s.ok(&["select", "--selector", "#cars", "--add", "audi"]);
    assert_eq!(r["selected"], serde_json::json!(["Volvo", "Audi"]), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#picked"]), "volvo,audi");
    // Without --add a select replaces the selection, as a plain click does.
    s.ok(&["select", "--selector", "#cars", "saab", "opel"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#picked"]), "saab,opel");

    s.ok(&["goto", &format!("{}/nbsp-select", site.url)]);
    s.ok(&["select", "--selector", "#city", "New York"]);
    assert_eq!(s.text(&["get", "value", "--selector", "#city"]), "ny");
    let err = s.fails(&["select", "--selector", "#city", "--add", "sf"]);
    assert!(err.contains("takes one option"), "{err}");
}

#[test]
fn the_element_wait_is_the_one_asked_for() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("wait", &["--wait-ms", "600"]);
    s.ok(&["goto", &format!("{}/slow?ms=1500", site.url)]);
    // The session's default applies, and the error says exactly that.
    let err = s.fails(&["click", "--selector", "#late"]);
    assert!(err.contains("within 600 ms"), "{err}");
    // A longer wait on one command.
    s.ok(&["click", "--selector", "#late", "--wait-ms", "4000"]);
    s.ok(&["wait", "--text", "late clicked"]);
}

#[test]
fn downloads_are_saved_and_waited_for() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("downloads", &[]);
    s.ok(&["goto", &format!("{}/download", site.url)]);
    s.ok(&["click", "--selector", "#dl"]);
    let r = s.ok(&["wait", "--download", "*.csv"]);
    let path = r["download"]["path"].as_str().expect("a path").to_string();
    assert!(path.ends_with("report.csv"), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "id,name\n1,Ada\n2,Grace\n");
    s.ok(&["click", "--text", "Save notes"]);
    let r = s.ok(&["wait", "--download", "notes.txt"]);
    assert_eq!(std::fs::read_to_string(r["download"]["path"].as_str().unwrap()).unwrap(), "notes from a blob\n");
    let list = s.ok(&["downloads", "--json"]);
    assert_eq!(list.as_array().map(Vec::len), Some(2), "{list}");
    s.ok(&["downloads", "--clear"]);
    let err = s.fails(&["wait", "--download", "--timeout-ms", "300"]);
    assert!(err.contains("no finished download"), "{err}");
    // A goto of a download is the download, once; the page stays.
    let r = s.ok(&["goto", &format!("{}/file.csv", site.url)]);
    assert_eq!(r["download"], "report.csv", "{r}");
    assert!(r["url"].as_str().unwrap_or("").ends_with("/download"), "{r}");
    s.ok(&["wait", "--download", "report*.csv"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(s.ok(&["downloads", "--json"]).as_array().map(Vec::len), Some(1));
    // A 204 is an answer, not a hiccup to retry.
    let err = s.fails(&["goto", &format!("{}/no-content", site.url)]);
    assert!(err.contains("204 (no content)"), "{err}");
}

#[test]
fn a_click_waits_for_a_sliding_modal_to_settle() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("modal", &[]);
    for _ in 0..2 {
        s.ok(&["goto", &format!("{}/entry-modal", site.url)]);
        s.ok(&["click", "--role", "button", "--name", "Close"]);
        // The press came after the page's own `transitionend` handler that
        // wires the button, not merely after the slide stopped.
        let order = s.ok(&["eval", "[window.wiredAt, window.pressedAt]"]);
        assert!(
            order[0].as_f64().unwrap_or(f64::MAX) <= order[1].as_f64().unwrap_or(0.0),
            "pressed before wired: {order}"
        );
        s.ok(&["wait", "--gone", "#modal"]);
    }
}

#[test]
fn a_click_waits_for_a_moving_target_to_stop() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("moving-target", &[]);
    s.ok(&["goto", &format!("{}/moving-target", site.url)]);
    s.ok(&["click", "--selector", "#mover"]);
    let status = s.text(&["get", "text", "--selector", "#status"]);
    // The button glides 40px a frame for 12 frames from x=60, then stops:
    // settled box centre x is 60 + 12*40 + 34/2 = 557. A click read before
    // it stopped lands on empty page and the status stays "waiting".
    assert!(status.starts_with("ok "), "click did not land on the button: {status}");
    let x: i32 = status
        .strip_prefix("ok ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("status carries the click x");
    assert!((540..=574).contains(&x), "click x {x} is not the settled centre 557");
}

#[test]
fn a_click_on_a_target_that_never_stops_still_lands() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("never-stops", &[]);
    s.ok(&["goto", &format!("{}/never-stops", site.url)]);
    let (ms, _) = timed(&s, &["click", "--selector", "#drifter"]);
    // About a second for the drift to stop, not the whole element wait.
    assert!(ms < 2500.0, "the click on a drifting button took {ms} ms");
    let status = s.text(&["get", "text", "--selector", "#status"]);
    // The drift is +/-12px around x=200, the button 60px wide: a click at
    // the last-read centre always lands on it once the settle budget ends
    // the stability wait. Reaching here at all proves there is no hang.
    assert!(status.starts_with("ok "), "click did not land on the button: {status}");
}

#[test]
fn a_field_under_a_panel_is_uncovered_before_typing() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("overlap", &[]);
    s.ok(&["goto", &format!("{}/overlapped", site.url)]);
    s.ok(&["fill", "--selector", "#name", "Prova"]);
    assert_eq!(s.text(&["get", "value", "--selector", "#name"]), "Prova");
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "ok");
}

#[test]
fn right_and_double_clicks() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("buttons", &[]);
    s.ok(&["goto", &format!("{}/right-click", site.url)]);
    s.ok(&["click", "--text", "Right Click Me", "--button", "right"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#msg"]), "You have done a right click");
    s.ok(&["dblclick", "--text", "Double Click Me"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#msg"]), "You have done a double click");
    let out = Command::new(tool_exe())
        .args(["--session", &s.name, "click", "--text", "Right Click Me", "--button", "back"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("left, right or middle"));
}

#[test]
fn tabs_by_target_id_and_requests_since() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("tabs", &[]);
    s.ok(&["goto", &format!("{}/right-click", site.url)]);
    s.ok(&["tab-new", &format!("{}/download", site.url)]);
    let tabs = s.ok(&["tab-list"]);
    let first = tabs["tabs"][0]["target"].as_str().expect("a target id")[..10].to_string();
    let r = s.ok(&["tab-select", &first]);
    assert!(r["url"].as_str().unwrap().ends_with("/right-click"), "{r}");

    let log = s.text(&["network"]);
    let last: u64 = log
        .lines()
        .filter_map(|l| l.strip_prefix('#')?.split_whitespace().next()?.parse().ok())
        .max()
        .expect("some requests");
    s.ok(&["goto", &format!("{}/multi-select", site.url)]);
    let since = s.text(&["network", "--since", &last.to_string()]);
    assert!(since.contains("/multi-select") && !since.contains("/right-click"), "{since}");
}

/// A tab closed with `tab-close` goes the way a user closes it: its page
/// gets to run its unload path (`beforeunload` included), so a tab that
/// hands work on from it (a leader telling the follower over a
/// BroadcastChannel) is heard at once. `Target.closeTarget` skipped that
/// path, and the follower never knew.
#[test]
fn closing_a_tab_lets_its_page_hand_over_to_another_tab() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("tabclose", &[]);
    s.ok(&["goto", &format!("{}/leader?leader", site.url)]);
    s.ok(&["tab-new", &format!("{}/leader", site.url)]);
    s.ok(&["tab-select", "0"]);
    s.ok(&["tab-close", "0"]);
    let waited = s.ok(&["wait", "--text", "took over", "--timeout-ms", "2000"]);
    assert!(waited["waited_ms"].as_f64().unwrap() < 2000.0, "the follower did not hear the leader go: {waited}");
}

/// A click on a window's own Close button succeeds and goes on in the tab
/// left, however the window's end and the click's answers interleave
/// (Electron's settings window, on a slow runner, came back as an error).
#[test]
fn a_click_that_closes_its_own_tab_succeeds() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("closer", &[]);
    s.ok(&["goto", &format!("{}/opener", site.url)]);
    for _ in 0..3 {
        s.ok(&["click", "--selector", "#open"]);
        let (success, closed) = s.run(&["click", "--selector", "#close"]);
        assert!(success && closed["ok"] == true, "{closed}");
        assert!(closed["notes"].to_string().contains("this click closed its tab"), "{closed}");
        assert_eq!(s.text(&["eval", "() => document.title"]), "opener");
    }
}

#[test]
fn an_op_after_a_late_reload_finds_the_next_page() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("wizard", &[]);
    for _ in 0..3 {
        s.ok(&["goto", &format!("{}/wizard", site.url)]);
        s.ok(&["fill", "--selector", "#name", "Ada"]);
        // The click saves with a request, and only then reloads: the next
        // op starts on the old page and has to carry on on the new one.
        s.ok(&["click", "--selector", "#next"]);
        s.ok(&["fill", "--selector", "#email", "ada@example.com"]);
        s.ok(&["click", "--selector", "#finish"]);
        assert_eq!(s.text(&["get", "text", "--selector", "#done"]), "Saved Ada / ada@example.com");
    }
}

#[test]
fn a_hidden_control_fails_fast_and_its_card_works() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("cards", &[]);
    s.ok(&["goto", &format!("{}/card-radios", site.url)]);
    let started = std::time::Instant::now();
    let err = s.fails(&["click", "--selector", "#in-s"]);
    assert!(err.contains("can't click"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "fails fast, not after the whole wait");
    assert_eq!(s.text(&["get", "text", "--selector", "#picked"]), "none");
    s.ok(&["click", "--role", "radio", "--name", "Medium"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#picked"]), "m");
}

/// A button that exists but is shown later (a dialog opening by a class, a
/// control made visible or clickable) is waited for, as a user would wait,
/// not failed after a moment: the click lands once it shows.
#[test]
fn a_click_waits_for_a_button_shown_later() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("shown-later", &[]);
    for (hide, show) in [
        ("display:none", "display:block"),
        ("visibility:hidden", "visibility:visible"),
        ("pointer-events:none", "pointer-events:auto"),
    ] {
        s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
        s.ok(&[
            "eval",
            &format!(
                "const b = document.createElement('button'); b.id = 'later'; b.textContent = 'Later'; \
                 b.style.cssText = '{hide}'; b.onclick = () => {{ b.textContent = 'clicked'; }}; \
                 document.body.prepend(b); setTimeout(() => {{ b.style.cssText = '{show}'; }}, 800); 1"
            ),
        ]);
        s.ok(&["click", "--selector", "#later"]);
        assert_eq!(s.text(&["get", "text", "--selector", "#later"]), "clicked", "{hide}");
    }
}

#[test]
fn select_works_on_an_aria_combobox() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("combo", &[]);
    s.ok(&["goto", &format!("{}/combobox", site.url)]);
    let r = s.ok(&["select", "--selector", "#cb", "Banana"]);
    assert_eq!(r["selected"], serde_json::json!(["Banana"]), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#committed"]), "Banana");
    // Not among the first three shown: typed to filter.
    s.ok(&["select", "--label", "Fruit", "cherry"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#committed"]), "Cherry");
    let err = s.fails(&["select", "--selector", "#cb", "Durian", "--timeout-ms", "1200"]);
    assert!(err.contains("no option \"Durian\""), "{err}");
}

#[test]
fn fill_keys_reaches_an_app_that_keeps_only_typing() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("keys", &[]);
    s.ok(&["goto", &format!("{}/typed-only", site.url)]);
    s.ok(&["fill", "--selector", "#amt", "123"]);
    s.ok(&["click", "--selector", "#go"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#model"]), "model=", "a set value is not typing");
    s.ok(&["fill", "--selector", "#amt", "--keys", "28036"]);
    s.ok(&["click", "--selector", "#go"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#model"]), "model=28036");
}

#[test]
fn scroll_to_bottom_scrolls_the_box_itself() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("lazybox", &[]);
    s.ok(&["goto", &format!("{}/lazy-box", site.url)]);
    // Each scroll to the box's end loads 20 more; wait for them rather than
    // a fixed pause (scroll events wait for a rendering step).
    for loaded in [40, 60] {
        let r = s.ok(&["scroll", "--selector", "#list", "--to", "bottom"]);
        assert!(r["scrollTop"].as_u64().unwrap() > 0, "{r}");
        s.ok(&["wait", "--js", &format!("document.querySelectorAll('#list div').length >= {loaded}")]);
    }
    s.ok(&["click", "--text", "Item 55"]);
    // Twice in a row, as a recorded session replays it: the second, at the
    // end already, waits for what the first loaded.
    s.ok(&["reload"]);
    s.ok(&["scroll", "--selector", "#list", "--to", "bottom"]);
    let r = s.ok(&["scroll", "--selector", "#list", "--to", "bottom"]);
    assert!(r["elementScrollHeight"].as_u64().unwrap() > 1000, "{r}");
    s.ok(&["click", "--text", "Item 55", "--timeout-ms", "2000"]);
}

#[test]
fn clicks_only_claim_what_a_user_could_do() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("honesty", &[]);
    s.ok(&["goto", &format!("{}/honesty", site.url)]);
    // Two #dup: the first is parked off the page, the second is the one shown.
    s.ok(&["click", "--selector", "#dup"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "real");
    let err = s.fails(&["click", "--selector", "#save", "--timeout-ms", "400"]);
    assert!(err.contains("disabled"), "{err}");
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "real");
    // Enabled after 600 ms: the click waits for it.
    s.ok(&["click", "--selector", "#unlock-later"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "unlocked");
    // By what it shows: the button, not the product card around it (its
    // aria-label says something longer).
    let r = s.ok(&["click", "--text", "Add to cart"]);
    assert!(r.get("no_effect").is_none(), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "added");
    let err = s.fails(&["fill", "--selector", "#ro", "x", "--timeout-ms", "300"]);
    assert!(err.contains("read-only"), "{err}");
    let err = s.fails(&["fill", "--selector", "#off", "x", "--timeout-ms", "300"]);
    assert!(err.contains("disabled"), "{err}");
}

#[test]
fn fill_waits_for_a_field_a_user_could_type_into() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("fillwait", &[]);
    s.ok(&["goto", &format!("{}/fillwait", site.url)]);
    let value = |sel: &str| s.text(&["eval", &format!("document.querySelector('{sel}').value")]);
    // Shown / enabled 400 ms after load (checked first, before any other wait): filled once ready.
    s.ok(&["fill", "--selector", "#later", "shown", "--timeout-ms", "3000"]);
    assert_eq!(value("#later"), "shown");
    s.ok(&["fill", "--selector", "#off", "enabled", "--timeout-ms", "3000"]);
    assert_eq!(value("#off"), "enabled");
    // Hidden: waits the whole timeout, then names the failed check.
    for sel in ["#hidden", "#vis"] {
        let t0 = std::time::Instant::now();
        let err = s.fails(&["fill", "--selector", sel, "x", "--timeout-ms", "700"]);
        assert!(err.contains("not visible"), "{err}");
        assert!(t0.elapsed() >= std::time::Duration::from_millis(650), "{:?}", t0.elapsed());
        assert_eq!(value(sel), "");
    }
    // Read-only never changes: fails as not editable.
    let err = s.fails(&["fill", "--selector", "#ro", "x", "--timeout-ms", "300"]);
    assert!(err.contains("not editable"), "{err}");
    // A transparent field is still one a user can click into.
    s.ok(&["fill", "--selector", "#zero", "ok", "--timeout-ms", "1000"]);
    assert_eq!(value("#zero"), "ok");
    // --force skips the checks.
    s.ok(&["fill", "--selector", "#hidden", "forced", "--force", "--timeout-ms", "300"]);
    assert_eq!(value("#hidden"), "forced");
}

#[test]
fn forms_inside_cross_origin_frames() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("xo", &[]);
    // Cross-site (localhost in a 127.0.0.1 page), then cross-origin on the
    // same site (another port).
    // By role, then by text: "Pay" in the frame matches exactly and wins
    // over the page's own "Payment details" heading and the frame's title.
    for (page, pay) in
        [("/xo-parent", ["--role", "button", "--name", "Pay"]), ("/xo-parent?same-site", ["--text", "Pay", "", ""])]
    {
        s.ok(&["goto", &format!("{}{page}", site.url)]);
        s.ok(&["fill", "--label", "Card number", "4242424242424242"]);
        s.ok(&["check", "--label", "Save card"]);
        s.ok(&["select", "--selector", "#exp", "02/28"]);
        let mut click = vec!["click"];
        click.extend(pay.iter().filter(|w| !w.is_empty()));
        let r = s.ok(&click);
        assert!(r.get("synthetic_click").is_none(), "{page}: {r}");
        // Text inside the frame, then the page's own (it hears by postMessage).
        s.ok(&["wait", "--text", "sent 4242424242424242"]);
        s.ok(&["wait", "--text", "paid 4242"]);
        assert_eq!(s.text(&["get", "text", "--selector", "#paid"]), "paid 4242 saved 02/28", "{page}");
        assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "sent 4242424242424242", "{page}");
    }
    let err = s.fails(&["click", "--text", "No such button", "--timeout-ms", "400"]);
    assert!(err.contains("cross-origin frame"), "{err}");
}

#[test]
fn a_click_that_changes_nothing_says_so() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("effects", &[]);
    s.ok(&["goto", &format!("{}/effects", site.url)]);
    // Close isn't wired yet: nothing happens, and the click says so instead
    // of a plain success.
    let r = s.ok(&["click", "--selector", "#late"]);
    let note = r["no_effect"].as_str().unwrap_or_default();
    assert!(note.contains("nothing on the page changed"), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "none");
    let r = s.ok(&["click", "--selector", "#noop"]);
    assert!(r.get("no_effect").is_some(), "{r}");
    // Once wired, the same click works and claims nothing extra.
    s.ok(&["eval", "wire()"]);
    let r = s.ok(&["click", "--selector", "#late"]);
    assert!(r.get("no_effect").is_none(), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "closed");
    // Effects a DOM observer alone would miss: a checkbox's state, a value
    // set from script, a request, a change a moment later, a scrolled strip,
    // a change inside another component's shadow root.
    for (target, what) in [
        ("#agree", "checkbox"),
        ("#clear", "field cleared from script"),
        ("#send", "request"),
        ("#later", "change 80 ms later"),
        ("#next", "scrolled strip"),
        ("#copy", "copied to the clipboard"),
        ("#clip", "clipboard write"),
    ] {
        let r = s.ok(&["click", "--selector", target]);
        assert!(r.get("no_effect").is_none(), "{what}: {r}");
    }
    assert_eq!(s.ok(&["get", "checked", "--selector", "#agree"]), true);
    assert_eq!(s.text(&["get", "value", "--selector", "#q"]), "");
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "later");
    let r = s.ok(&["click", "--role", "button", "--name", "Bump"]);
    assert!(r.get("no_effect").is_none(), "shadow root: {r}");
    // Typing into a field or opening a <select> isn't judged.
    let r = s.ok(&["click", "--selector", "#q"]);
    assert!(r.get("no_effect").is_none(), "field: {r}");

    // A page that reports every click to analytics: the report alone isn't
    // an effect, and the note says what was sent.
    s.ok(&["goto", &format!("{}/effects?telemetry", site.url)]);
    let r = s.ok(&["click", "--selector", "#noop"]);
    let note = r["no_effect"].as_str().unwrap_or_default();
    assert!(note.contains("only sent GET /event/click (200)"), "{r}");
    // Clicks that do something still say nothing extra there: a save whose
    // result shows when the server answers, a checkbox.
    let r = s.ok(&["click", "--selector", "#save"]);
    assert!(r.get("no_effect").is_none(), "slow save: {r}");
    s.ok(&["wait", "--text", "saved"]);
    let r = s.ok(&["click", "--selector", "#agree"]);
    assert!(r.get("no_effect").is_none(), "checkbox: {r}");
}

/// `show()` and `showPopover()` queue their `toggle` as a later task, so an
/// earlier click's can land while the next click watches for its effect: a
/// toggle saying a dialog or popover went to the state it already had is
/// not that click's, and a click that did nothing still says so. Opening
/// and closing a popover (no attribute changes, only the toggle) still count.
#[test]
fn a_toggle_an_earlier_click_queued_is_not_the_next_clicks_effect() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("toggles", &[]);
    s.ok(&["goto", &format!("{}/toggles", site.url)]);
    for target in ["#open-tip", "#close-tip", "#open-tip"] {
        let r = s.ok(&["click", "--selector", target]);
        assert!(r.get("no_effect").is_none(), "{target}: {r}");
    }
    for target in ["#late-tip", "#late-dlg", "#noop"] {
        let r = s.ok(&["click", "--selector", target]);
        assert!(r.get("no_effect").is_some(), "{target}: {r}");
    }
}

/// A script that clicks right after `goto` can beat the page to wiring the
/// button up: a timer after load, a script the page loads then, a listener
/// on the button's container or on the document. Nothing listened for that
/// click, so once the page wires the element up it is clicked again, and
/// counts once. A click something heard is never sent again: not when the
/// page adds a listener later, not when analytics on the document heard it,
/// and not when a script there recorded it to replay once the button is
/// wired, even a replay that shows nothing. A button never wired still says
/// the click did nothing.
#[test]
fn a_click_before_the_page_wired_the_element_is_sent_again() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("wired-late", &[]);
    let steps = std::env::temp_dir().join(format!("nv-wired-late-{}.txt", std::process::id()));
    let steps_arg = steps.display().to_string();
    // `again`: Some(true) the click must have been sent again (the script
    // that wires the button takes 1.5 s to load, so the click is always
    // first), Some(false) it must not have been, None either way: a click
    // that lands after a timer already wired the button (a loaded runner)
    // just works. Whatever the path, it counts once.
    for (query, target, out, hits, again) in [
        ("script=1500", "#go", "went 1", 1, Some(true)),
        ("ms=400", "#go", "went 1", 1, None),
        ("ms=150", "#go", "went 1", 1, None),
        ("root=400", "#inner", "inner 1", 1, None),
        ("doc=400", "#go", "went 1", 1, None),
        ("analytics&ms=400", "#go", "idle", 0, Some(false)),
        ("replay=400", "#go", "went 1", 1, Some(false)),
        ("replay=400&late", "#go", "went 1", 1, Some(false)),
        ("replay=400&invisible", "#go", "idle", 1, Some(false)),
        ("quiet", "#go", "idle", 1, Some(false)),
        ("ms=99999", "#go", "idle", 0, Some(false)),
    ] {
        // goto and click in one call: no pause between them.
        std::fs::write(&steps, format!("goto {}/wired-late?{query}\nclick --selector {target}\n", site.url)).unwrap();
        let r = s.ok(&["batch", &steps_arg]);
        let click = r["results"]
            .as_array()
            .and_then(|rs| rs.iter().find(|x| x["do"].as_str().is_some_and(|d| d.starts_with("click"))))
            .map(|x| x["result"].clone())
            .unwrap_or_default();
        let sent_again = click.get("clicked_again").is_some();
        if let Some(expected) = again {
            assert_eq!(sent_again, expected, "{query}: {r}");
        }
        // A click sent again reports the second one's effect; one that
        // never had any says so. (A replayed click is seen only later.)
        if sent_again {
            assert!(click.get("no_effect").is_none(), "{query}: {r}");
        }
        if out == "idle" {
            assert!(click.get("no_effect").is_some(), "{query}: {r}");
        }
        // A page that replays a recorded click does it after the click
        // returned; navigera's own clicks are all done by then.
        if hits > 0 {
            s.ok(&["wait", "--js", "window.hits > 0"]);
        }
        assert_eq!(s.text(&["get", "text", "--selector", "#out"]), out, "{query}: {r}");
        assert_eq!(s.ok(&["eval", "window.hits"]), hits, "{query}: {r}");
    }
    let _ = std::fs::remove_file(&steps);
}

#[test]
fn reset_starts_clean_in_the_same_browser() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("reset", &[]);
    s.ok(&["goto", &format!("{}/effects", site.url)]);
    s.ok(&["eval", "(localStorage.setItem('k', 'v'), document.cookie = 'c=1; path=/', 1)"]);
    s.ok(&["tab-new", &format!("{}/honesty", site.url)]);
    let r = s.ok(&["reset"]);
    assert_eq!(r["tabs"].as_array().map(Vec::len), Some(1), "{r}");
    s.ok(&["goto", &format!("{}/effects", site.url)]);
    assert_eq!(s.ok(&["eval", "[localStorage.getItem('k'), document.cookie]"]), serde_json::json!([null, ""]));
    // Cookies set in the new context are the ones `cookies --all` lists.
    s.ok(&["eval", "(document.cookie = 'fresh=1; path=/', 1)"]);
    let all = s.ok(&["cookies", "--all", "--json"]);
    let names: Vec<&str> = all.as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(names, ["fresh"], "{all}");
    // Downloads still land where they should in the new context.
    s.ok(&["goto", &format!("{}/download", site.url)]);
    s.ok(&["click", "--selector", "#dl"]);
    let r = s.ok(&["wait", "--download", "*.csv"]);
    assert!(r["download"]["path"].as_str().unwrap_or_default().ends_with("report.csv"), "{r}");
    // Twice in a row: the first context goes away with its tabs.
    s.ok(&["reset"]);
    assert_eq!(s.ok(&["tab-list"])["tabs"].as_array().map(Vec::len), Some(1));
}

/// The browser's own list of targets (`/json/list` on its debugging port):
/// (type, url) of each.
fn browser_targets(port: u16) -> Vec<(String, String)> {
    use std::io::{Read, Write};
    let Ok(mut c) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return Vec::new() };
    let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    // HTTP/1.1 (Chrome's debugging server leaves a 1.0 request unanswered),
    // and read to the length it gives: it keeps the connection open.
    let _ = c.write_all(format!("GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
    let mut got = Vec::new();
    let mut buf = [0u8; 8192];
    let body = loop {
        let Ok(n) = c.read(&mut buf) else { return Vec::new() };
        if n == 0 {
            return Vec::new();
        }
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got);
        let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
        let len = head
            .lines()
            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>()))
            .and_then(Result::ok)
            .unwrap_or(0);
        if body.len() >= len {
            break body.to_string();
        }
    };
    let list: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|t| (t["type"].as_str().unwrap_or_default().into(), t["url"].as_str().unwrap_or_default().into()))
                .collect()
        })
        .unwrap_or_default()
}

/// A scenario runner resets between tests; each reset used to open the new
/// tab before closing the old one, so both pages' renderers were alive at
/// once, and each new context loaded the address bar's popups into a
/// renderer of their own in a browser that shows no address bar. Both were
/// the peak of a test run's memory. Watched from the browser's side.
#[test]
fn a_reset_never_has_two_pages_and_a_headless_browser_loads_no_address_bar() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let name = std::env::temp_dir()
        .join(format!("nv-interactions-twopages-{}.sock", std::process::id()))
        .display()
        .to_string();
    let start = Command::new(tool_exe())
        .args(["--session", &name, "start"])
        .env("NAVIGERA_EXTRA_FLAGS", format!("--remote-debugging-port={port}"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(start.status.success(), "{}", String::from_utf8_lossy(&start.stderr));
    let s = Session { name, _downloads: None };
    s.ok(&["goto", &format!("{}/effects", site.url)]);
    let pages = |t: &[(String, String)]| t.iter().filter(|(kind, _)| kind == "page").count();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut most = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                most = most.max(pages(&browser_targets(port)));
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            most
        })
    };
    for _ in 0..5 {
        s.ok(&["reset"]);
        s.ok(&["goto", &format!("{}/effects", site.url)]);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(watch.join().unwrap(), 1, "a reset had the old and the new tab open at once");
    let targets = browser_targets(port);
    assert_eq!(pages(&targets), 1, "{targets:?}");
    let ui: Vec<_> = targets.iter().filter(|(kind, url)| kind != "page" && url.contains("omnibox")).collect();
    assert!(ui.is_empty(), "a headless browser loaded the address bar's popups: {ui:?}");
}

#[test]
fn the_mouse_goes_where_it_is_told_on_a_canvas() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("mouse", &[]);
    s.ok(&["goto", &format!("{}/canvas", site.url)]);
    let log = |s: &Session| -> Vec<String> {
        let v = s.ok(&["eval", "log.splice(0)"]);
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    };
    // From the canvas's corner: a press, a path, a release draws a line.
    let r = s.ok(&["mouse", "move", "10", "20", "--selector", "canvas"]);
    assert_eq!((r["mouse"]["x"].as_f64(), r["mouse"]["y"].as_f64()), (Some(60.0), Some(60.0)), "{r}");
    s.ok(&["mouse", "down"]);
    s.ok(&["mouse", "move", "110", "20", "--selector", "canvas", "--steps", "10"]);
    let r = s.ok(&["mouse", "up"]);
    assert_eq!(r["mouse"]["held"], serde_json::json!([]), "{r}");
    let events = log(&s);
    assert_eq!(events.first().map(String::as_str), Some("move 10 20 0"), "{events:?}");
    assert!(events.contains(&"mousedown 10 20 0 1".to_string()), "{events:?}");
    assert!(events.contains(&"move 60 20 1".to_string()), "{events:?}");
    assert_eq!(events.last().map(String::as_str), Some("click 110 20 0 0"), "{events:?}");
    assert!(events.contains(&"mouseup 110 20 0 0".to_string()), "{events:?}");
    // Every segment was drawn; the pixels show it unless the platform lost
    // the canvas's context meanwhile (macOS under load; it is never restored).
    assert_eq!(s.ok(&["eval", "drawn"]), 10, "a stroke per move with the button down");
    if s.ok(&["eval", "lost.length"]) == 0 {
        assert!(s.ok(&["eval", "ink(60, 20)"]).as_u64().unwrap_or(0) > 0, "the line is drawn");
    }
    assert_eq!(s.ok(&["eval", "ink(60, 120)"]), 0, "nothing drawn off the path");
    // Viewport coordinates, a double click, the right button, the wheel.
    s.ok(&["mouse", "click", "250", "140", "--count", "2"]);
    let events = log(&s);
    assert!(events.contains(&"dblclick 200 100 0 0".to_string()), "{events:?}");
    s.ok(&["mouse", "click", "30", "30", "--selector", "canvas", "--button", "right"]);
    let events = log(&s);
    assert!(events.contains(&"mousedown 30 30 2 2".to_string()), "{events:?}");
    // The menu opens on the press (Linux, macOS) or the release (Windows).
    assert!(events.iter().any(|e| e.starts_with("contextmenu 30 30 2 ")), "{events:?}");
    s.ok(&["mouse", "wheel", "0", "-120"]);
    assert_eq!(log(&s), ["wheel 0 -120"]);
    // A target scrolled out of view is brought back, as a click would; down,
    // up and wheel with a target go to its center.
    s.ok(&["eval", "(scrollTo(0, 1500), 1)"]);
    s.ok(&["mouse", "click", "15", "25", "--selector", "canvas"]);
    assert!(log(&s).contains(&"click 15 25 0 0".to_string()));
    s.ok(&["eval", "(scrollTo(0, 1500), 1)"]);
    s.ok(&["mouse", "down", "--selector", "canvas"]);
    s.ok(&["mouse", "up"]);
    assert!(log(&s).contains(&"mousedown 150 100 0 1".to_string()));
    // A click where the page isn't would land on nothing: refused.
    assert!(s.fails(&["mouse", "click", "10", "5000"]).contains("outside the viewport"));
    assert!(s.fails(&["mouse", "click", "10", "9000", "--selector", "canvas"]).contains("outside the viewport"));
    // Mistakes are argument errors, said before anything runs.
    let says = |args: &[&str]| {
        let out = Command::new(tool_exe()).arg("--session").arg(&s.name).args(args).output().unwrap();
        assert!(!out.status.success());
        String::from_utf8_lossy(&out.stderr).to_string()
    };
    assert!(says(&["mouse", "jump", "1", "2"]).contains("move, down, up, click or wheel"));
    assert!(says(&["mouse", "move", "5"]).contains("no y"));
}

#[test]
fn frame_scopes_pick_the_frame_when_ids_repeat() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("frame-scopes", &[]);
    s.ok(&["goto", &format!("{}/twin-frames", site.url)]);
    // Two frames of this origin with the same form.
    s.ok(&["fill", "--selector", "#name", "--frame", "#billing", "Ada"]);
    s.ok(&["click", "--text", "Save", "--frame", "#billing"]);
    assert_eq!(s.text(&["get", "value", "--selector", "#name", "--frame", "#billing"]), "Ada");
    assert_eq!(s.text(&["get", "value", "--selector", "#name", "--frame", "#shipping"]), "");
    s.ok(&["fill", "--label", "Name", "--frame", "#shipping", "Bo"]);
    s.ok(&["click", "--role", "button", "--name", "Save", "--frame", "#shipping"]);
    // Two of another origin with the same card form: the second one.
    s.ok(&["fill", "--selector", "#cardnumber", "--frame", "#card2", "4242424242424242"]);
    let r = s.ok(&["click", "--selector", "#pay", "--frame", "#card2"]);
    assert!(r.get("synthetic_click").is_none(), "{r}");
    // The page hears it by postMessage, from another process: wait for it.
    s.ok(&["wait", "--text", "card2:4242"]);
    // A frame inside a frame.
    s.ok(&["click", "--text", "Deep", "--frame", "#outer >> #inner"]);
    s.ok(&["wait", "--text", "deep"]);
    // Each action landed in the frame it named, and only there.
    let log = s.text(&["get", "text", "--selector", "#log"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#status", "--frame", "#card"]), "idle");
    assert!(log.starts_with("billing:Ada shipping:Bo card2:4242"), "{log}");
    assert!(log.trim_end().ends_with("deep"), "{log}");
    assert!(!log.contains("card:"), "{log}");
    // Waits look in the frame they name, of either origin.
    s.ok(&["wait", "--selector", "#name", "--frame", "#billing"]);
    s.ok(&["wait", "--text", "sent 4242424242424242", "--frame", "#card2"]);
    let err = s.fails(&["wait", "--text", "sent", "--frame", "#card", "--timeout-ms", "300"]);
    assert!(err.contains("in frame \"#card\" not seen"), "{err}");
    // Mistakes say what's wrong.
    let err = s.fails(&["click", "--text", "Save", "--frame", "#nowhere", "--timeout-ms", "300"]);
    assert!(err.contains("no frame matches --frame \"#nowhere\""), "{err}");
    let err = s.fails(&["fill", "--selector", "#name", "--frame", "#billing >> #nope", "x", "--timeout-ms", "300"]);
    assert!(err.contains("no frame matches"), "{err}");
    let err = s.fails(&["click", "--text", "Save", "--frame", "#log"]);
    assert!(err.contains("not a frame"), "{err}");
}

#[test]
fn a_page_that_stops_answering_costs_one_timeout_not_one_per_command() {
    if !chrome() {
        return;
    }
    let s = Session::start("stalled", &["--timeout-ms", "10000"]);
    // The button's mousedown holds the page's thread for 25 s.
    let page = "<title>Busy</title><button onmousedown=\"const e = Date.now() + 25000; while (Date.now() < e);\">Hold</button>";
    // The short session timeout is for the stall; loading the page on a
    // loaded CI runner gets the usual one (a data URL took 12 s on macOS).
    s.ok(&["goto", &format!("data:text/html,{page}"), "--timeout-ms", "35000"]);
    let started = std::time::Instant::now();
    let err = s.fails(&["click", "--text", "Hold"]);
    let click = started.elapsed();
    assert!(err.contains("did not answer"), "{err}");
    // One 10 s timeout for the press, then seconds for the clean-up (it
    // took two full timeouts, 20 s, when every command waited 10 s).
    assert!(click < std::time::Duration::from_secs(17), "click took {click:?}");
    let started = std::time::Instant::now();
    s.fails(&["title"]);
    assert!(started.elapsed() < std::time::Duration::from_secs(7), "title took {:?}", started.elapsed());
    // Once the page answers again, commands get their whole timeout back.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let (ok, _) = s.run(&["title"]);
        if ok {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the page never answered again");
    }
    assert_eq!(s.ok(&["eval", "new Promise(r => setTimeout(() => r(7), 4000))"]), 7);
}

/// The ref of the first `ax` line holding `text`.
fn ref_of(s: &Session, text: &str) -> String {
    let tree = s.text(&["ax"]);
    let line = tree.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no {text:?} in\n{tree}"));
    line.split("ref=").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).unwrap().to_string()
}

#[test]
fn locate_and_history_turn_refs_into_locators() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("history", &[]);
    s.ok(&["goto", &format!("{}/iframe-form", site.url)]);
    // Each locator found the element; inside a frame, the frame is named.
    let email = ref_of(&s, "textbox \"Email\"");
    let found = s.ok(&["locate", &email]);
    let flags: Vec<Vec<String>> = found["locators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["flags"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect())
        .collect();
    assert_eq!(flags[0], ["--role", "textbox", "--name", "Email", "--frame", "#email-subscribe"], "{found}");
    assert!(flags.iter().any(|f| f[..2] == ["--selector", "#email"]), "{found}");
    for f in &flags {
        let mut args = vec!["get", "value"];
        args.extend(f.iter().map(String::as_str));
        s.ok(&args);
    }
    // A name two elements share is not offered; a path is.
    s.ok(&["eval", "(document.body.insertAdjacentHTML('beforeend', '<button>Go</button><button>Go</button>'), 1)"]);
    let second = s.ok(&["eval", "document.querySelectorAll('button').length"]);
    assert_eq!(second, 2);
    let go = s.text(&["ax"]).lines().filter(|l| l.contains("button \"Go\"")).nth(1).unwrap().to_string();
    let go = go.split("ref=").nth(1).unwrap().trim_end_matches(']').to_string();
    let found = s.ok(&["locate", &go]);
    let kinds: Vec<&str> = found["locators"].as_array().unwrap().iter().map(|l| l["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["css"], "{found}");
    assert_eq!(found["locators"][0]["flags"][1], "body > button:nth-of-type(2)", "{found}");
    // The same name in two cards: the one near its card's heading.
    s.ok(&["eval", "(document.body.insertAdjacentHTML('beforeend', '<div><h3>Nickel Widget</h3><p>$5</p><button onclick=\"window.added=1\">Add to cart</button></div><div><h3>Brass Widget</h3><p>$7</p><button onclick=\"window.added=2\">Add to cart</button></div>'), 1)"]);
    let brass = s.text(&["ax"]).lines().filter(|l| l.contains("button \"Add to cart\"")).nth(1).unwrap().to_string();
    let brass = brass.split("ref=").nth(1).unwrap().trim_end_matches(']').to_string();
    let found = s.ok(&["locate", &brass]);
    assert_eq!(found["locators"][0]["kind"], "near", "{found}");
    let flags: Vec<String> =
        found["locators"][0]["flags"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
    assert_eq!(flags, ["--role", "button", "--name", "Add to cart", "--near", "Brass Widget"], "{found}");
    let mut click = vec!["click"];
    click.extend(flags.iter().map(String::as_str));
    s.ok(&click);
    assert_eq!(s.ok(&["eval", "window.added"]), 2);
    // --near on its own is refused when the words are read (exit 2, no
    // browser asked): it narrows a locator.
    let out = Command::new(tool_exe())
        .args(["--session", &s.name, "click", "--near", "Brass Widget"])
        .stdin(Stdio::null())
        .output()
        .expect("run navigera");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.code() == Some(2) && err.contains("--near narrows"), "{err}");
    // A select inside its label is named by the label's words, not its options.
    s.ok(&["eval", "(document.body.insertAdjacentHTML('beforeend', '<label>Size <select><option>Small</option><option>Large</option></select></label>'), 1)"]);
    let size = ref_of(&s, "combobox \"Size\"");
    let found = s.ok(&["locate", &size]);
    assert_eq!(found["locators"][0]["flags"], serde_json::json!(["--role", "combobox", "--name", "Size"]), "{found}");
    s.ok(&["select", "--label", "Size", "Large"]);
    // A password field says so, whatever it is called.
    s.ok(&["eval", "(document.body.insertAdjacentHTML('beforeend', '<label>Token <input type=password></label>'), 1)"]);
    let token = ref_of(&s, "textbox \"Token\"");
    assert_eq!(s.ok(&["locate", &token])["secret"], true);
    assert!(s.ok(&["locate", &size]).get("secret").is_none());
    // The actions, a ref's with its locators read before it ran; reads aren't kept.
    s.ok(&["history", "--clear"]);
    s.ok(&["fill", &email, "ada@example.com"]);
    s.ok(&["click", "--text", "Subscribe"]);
    s.ok(&["get", "text", "--selector", "#msg"]);
    s.ok(&["wait", "--text", "Thanks"]);
    let h = s.ok(&["history"]);
    let h = h["history"].as_array().unwrap();
    assert_eq!(h.len(), 3, "reads aren't kept, the wait is: {h:?}");
    assert_eq!(h[2]["command"]["op"], "wait", "{h:?}");
    assert_eq!(
        (h[0]["command"]["op"].as_str(), h[0]["command"]["value"].as_str()),
        (Some("fill"), Some("ada@example.com"))
    );
    assert_eq!(h[0]["locators"][0]["flags"][1], "textbox", "{h:?}");
    assert_eq!(h[1]["command"]["text"], "Subscribe", "{h:?}");
    assert!(h[1].get("locators").is_none() && h[1]["url"].as_str().unwrap().ends_with("/iframe-form"), "{h:?}");
}

#[test]
fn an_element_in_a_shadow_root_is_located_through_its_hosts() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("shadow-hosts", &[]);
    s.ok(&["goto", &format!("{}/iframe-form", site.url)]);
    // Two cards inside an app, each with an icon button and no name for it.
    s.ok(&[
        "eval",
        r#"(customElements.define('x-card', class extends HTMLElement { connectedCallback() { if (!this.shadowRoot) this.attachShadow({ mode: 'open' }).innerHTML = '<span>card</span><button onclick="window.picked = this.getRootNode().host.title"><svg width=12 height=12></svg></button>'; } }),
           customElements.define('x-app', class extends HTMLElement { connectedCallback() { if (!this.shadowRoot) this.attachShadow({ mode: 'open' }).innerHTML = '<x-card title=Nickel></x-card><x-card title=Brass></x-card>'; } }),
           document.body.insertAdjacentHTML('beforeend', '<button>Decoy</button><x-app id=app></x-app>'), 1)"#,
    ]);
    assert_eq!(s.ok(&["get", "count", "#app >>> x-card >>> button"]), 2);
    assert_eq!(s.ok(&["get", "count", "#app >>> button"]), 2, "a shadow root under the host's counts");
    assert_eq!(s.ok(&["get", "count", "#app >>> nothing"]), 0);
    let found = s.ok(&["locate", "--selector", "#app >>> x-card:nth-of-type(2) >>> button"]);
    assert_eq!(
        found["locators"],
        serde_json::json!([{"kind": "css", "flags": ["--selector", "#app >>> x-card:nth-of-type(2) >>> button"]}]),
        "{found}"
    );
    s.ok(&["click", "--selector", "#app >>> x-card:nth-of-type(2) >>> button"]);
    assert_eq!(s.ok(&["eval", "window.picked"]), "Brass");
}

#[test]
fn what_a_page_made_clickable_has_a_ref_that_clicks() {
    if !chrome() {
        return;
    }
    let s = Session::start("clickable", &[]);
    let page = "<title>Inbox</title><style>.thread{cursor:pointer} #bar span{display:inline-block;width:20px;height:20px}</style>\
        <div id=app><div class=thread><div>Kelcey</div><div>Quisque</div></div><div class=thread><div>Gilli</div><div>Quam.</div></div>\
        <div id=bar><span id=close-forward></span><span id=send-forward></span></div></div>\
        <label for=f style=cursor:pointer>Attachment</label><input type=file id=f>\
        <div style=\"cursor:pointer;pointer-events:none\" onclick=\"document.title='passed through'\">Pass-through</div>\
        <span id=empty onclick=\"document.title='empty'\"></span>\
        <div id=guard style=\"position:absolute;left:0;top:600px;width:200px;height:100px;opacity:0\" onclick=\"document.title='guard'\"></div>\
        <script>for (const t of document.querySelectorAll('.thread')) t.onclick = () => document.title = 'open ' + t.textContent;\
        for (const id of ['close-forward', 'send-forward']) document.getElementById(id).onclick = () => document.title = id;\
        document.getElementById('app').addEventListener('click', () => {});</script>";
    s.ok(&["goto", &format!("data:text/html,{}", page.replace('#', "%23"))]);
    let ax = s.text(&["ax"]);
    let ref_of = |line_start: &str| -> String {
        let line =
            ax.lines().find(|l| l.trim_start().starts_with(line_start)).unwrap_or_else(|| panic!("{line_start}: {ax}"));
        line.split("ref=")
            .nth(1)
            .and_then(|r| r.split(']').next())
            .unwrap_or_else(|| panic!("no ref: {line}"))
            .to_string()
    };
    // The row and the icon with no text get refs; the container that only
    // listens for its children doesn't.
    let row = ref_of("- clickable [ref=");
    assert!(ax.contains("Gilli Quam."), "{ax}");
    let send = ref_of("- clickable #send-forward");
    assert!(!ax.contains("#app"), "{ax}");
    // A label clicks its control, which is listed itself.
    assert!(!ax.lines().any(|l| l.contains("clickable") && l.contains("Attachment")), "{ax}");
    // What a user's click can't land on isn't offered: clicks pass
    // through it, it has no size, or it can't be seen.
    for what in ["Pass-through", "#empty", "#guard"] {
        assert!(!ax.lines().any(|l| l.contains("clickable") && l.contains(what)), "{what}: {ax}");
    }
    s.ok(&["click", &send]);
    assert_eq!(s.ok(&["title"]), "send-forward");
    s.ok(&["click", &row]);
    assert_eq!(s.ok(&["title"]), "open KelceyQuisque");
}

#[test]
fn a_list_that_listens_for_its_items_marks_the_items() {
    if !chrome() {
        return;
    }
    let s = Session::start("delegated", &[]);
    // One listener on the list or table, for whichever item is clicked: a
    // ref on the list would click the middle item. A row's own listener,
    // and a button made of an icon and a label, stay one ref.
    let page = "<title>Fruit</title>\
        <ul onclick=\"if (event.target.tagName == 'LI') document.title = 'picked ' + event.target.textContent\">\
        <li>Apple</li><li>Banana</li><li>Cherry</li></ul>\
        <table onclick=\"const r = event.target.closest('tr'); if (r) document.title = 'row ' + r.rowIndex\">\
        <tr><td>a</td><td>1</td></tr><tr><td>b</td><td>2</td></tr></table>\
        <table><tr onclick=\"document.title = 'own row'\"><td>x</td><td>y</td></tr></table>\
        <div onclick=\"document.title = 'saved'\"><span class=icon></span><span>Save</span></div>";
    s.ok(&["goto", &format!("data:text/html,{page}")]);
    let ax = s.text(&["ax"]);
    // The tree, without its first line (the page's URL holds the markup).
    let tree: Vec<&str> = ax.lines().skip(1).collect();
    let ref_in = |line: &str| line.split("ref=").nth(1).and_then(|r| r.split(']').next()).unwrap().to_string();
    let ref_of = |what: &str| {
        ref_in(tree.iter().find(|l| l.contains(what) && l.contains("ref=")).unwrap_or_else(|| panic!("{what}: {ax}")))
    };
    assert!(!tree.iter().any(|l| l.trim_start().starts_with("- list [")), "{ax}");
    assert_eq!(tree.iter().filter(|l| l.contains("listitem") && l.contains("ref=")).count(), 3, "{ax}");
    // An item's ref and its text are on one line, its listitem's.
    let item_ref = |fruit: &str| {
        ref_in(
            tree.iter()
                .find(|l| l.contains("listitem") && l.ends_with(&format!(": {fruit}")))
                .unwrap_or_else(|| panic!("{fruit}: {ax}")),
        )
    };
    s.ok(&["click", &item_ref("Cherry")]);
    assert_eq!(s.ok(&["title"]), "picked Cherry");
    s.ok(&["click", &item_ref("Apple")]);
    assert_eq!(s.ok(&["title"]), "picked Apple");
    s.ok(&["click", &ref_of("b 2")]);
    assert_eq!(s.ok(&["title"]), "row 1");
    assert_eq!(tree.iter().filter(|l| l.contains("x y") && l.contains("ref=")).count(), 1, "{ax}");
    assert_eq!(tree.iter().filter(|l| l.contains("Save") && l.contains("ref=")).count(), 1, "{ax}");
    s.ok(&["click", &ref_of("Save")]);
    assert_eq!(s.ok(&["title"]), "saved");
}

#[test]
fn clickable_refs_skip_what_a_user_cant_tell_apart_or_reach() {
    if !chrome() {
        return;
    }
    let s = Session::start("clickable-real", &[]);
    // An image card inside an article link (the link has the ref); a
    // handler kept from the user with aria-hidden; a row of blank tiles
    // with nothing to tell them by. A menu label whose real checkbox is
    // hidden is the thing a user clicks, so it gets the ref.
    let tile = "<div style=\"display:inline-block;width:20px;height:20px;cursor:pointer\"></div>";
    let page = format!(
        "<title>News</title>\
        <a href=\"%23story\"><div class=card onclick=\"void 0\"><span>Story picture</span></div>Story</a>\
        <div aria-hidden=true><span onclick=\"document.title = 'hidden'\">Behind the modal</span></div>\
        <div>{tile}{tile}{tile}</div>\
        <div role=button tabindex=0 aria-label=\"Insert page\"><span class=icon onclick=\"void 0\">+</span></div>\
        <span class=stepper style=\"visibility:hidden\" onclick=\"void 0\">Up</span>\
        <input type=checkbox id=menu aria-hidden=true tabindex=-1 style=\"position:absolute;opacity:0\" onchange=\"document.title = 'menu ' + this.checked\">\
        <label for=menu style=cursor:pointer>Menu</label>"
    );
    s.ok(&["goto", &format!("data:text/html,{page}")]);
    let ax = s.text(&["ax"]);
    let tree: Vec<&str> = ax.lines().skip(1).collect();
    let clickable: Vec<&&str> = tree.iter().filter(|l| l.contains("clickable")).collect();
    assert!(!clickable.iter().any(|l| l.contains("Story picture") || l.contains(".card")), "{ax}");
    assert!(!clickable.iter().any(|l| l.contains("Behind the modal")), "{ax}");
    // Inside a role=button, which has the ref; hidden until hovered.
    assert!(!clickable.iter().any(|l| l.contains(".icon") || l.contains(".stepper") || l.contains("Up")), "{ax}");
    assert!(tree.iter().any(|l| l.contains("button \"Insert page\"") && l.contains("ref=")), "{ax}");
    assert!(!tree.iter().any(|l| l.trim() == "- clickable" || l.trim_start().starts_with("- clickable [ref=")), "{ax}");
    let menu = clickable.iter().find(|l| l.contains("Menu")).unwrap_or_else(|| panic!("no ref on the label: {ax}"));
    let menu = menu.split("ref=").nth(1).and_then(|r| r.split(']').next()).unwrap();
    s.ok(&["click", menu]);
    assert_eq!(s.ok(&["title"]), "menu true");
}

/// Order of the list items on a drag page, e.g. "bca".
fn list_order(s: &Session) -> String {
    s.text(&["eval", "[...document.querySelectorAll('li')].map(l => l.id).join('')"])
}

/// `drag` drives a real drag: the browser starts it from the mouse press, and
/// the drag events follow the pointer to the drop. Desktop and touch emulation.
#[test]
fn drag_runs_a_real_html5_drag() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    for (tag, mobile) in [("desk", false), ("mob", true)] {
        let s = Session::start(&format!("dragn-{tag}"), &[]);
        if mobile {
            s.ok(&["set", "device", "iPhone 15"]);
        }
        for page in ["drag-native", "drag-pointer"] {
            s.ok(&["goto", &format!("{}/{page}", site.url)]);
            let r = s.ok(&["drag", "#a", "#c"]);
            assert_eq!(r["drag"]["path"], "drag", "{tag} {page}: {r}");
            assert_eq!(r["drag"]["html5"], true, "{tag} {page}: {r}");
            assert_eq!(r["drag"]["dropped"], true, "{tag} {page}: {r}");
            assert_eq!(list_order(&s), "bca", "{tag} {page}");
            // The browser ran the whole drag: dragend fired, and dragenter
            // followed the pointer over more than the target.
            let log = s.text(&["get", "text", "--selector", "#log"]);
            assert!(log.starts_with("start enter"), "{tag} {page}: {log}");
            assert!(log.matches("enter").count() >= 2, "{tag} {page}: {log}");
            assert!(log.ends_with("drop:a end"), "{tag} {page}: {log}");
            // The mouse is released: a click right after is a plain click.
            s.ok(&["click", "#b"]);
        }
        // Dropped on something that takes no drop: the drag ran, nothing landed.
        s.ok(&["goto", &format!("{}/drag-native", site.url)]);
        s.ok(&["eval", "document.getElementById('log').textContent = 'no drop here'"]);
        let r = s.ok(&["drag", "#a", "#log"]);
        assert_eq!(r["drag"]["path"], "drag", "{tag}: {r}");
        assert_eq!(r["drag"]["dropped"], false, "{tag}: {r}");
        assert_eq!(list_order(&s), "abc", "{tag}");
    }
}

/// A target that has to scroll into view: the source is pressed where it is
/// before that scroll, not where it was (the scroll moves it away, and the
/// old point then lies over the target itself).
#[test]
fn drag_to_a_target_far_below_drags_the_source() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    for (tag, mobile) in [("desk", false), ("mob", true)] {
        let s = Session::start(&format!("dragf-{tag}"), &[]);
        if mobile {
            s.ok(&["set", "device", "iPhone 15"]);
        }
        for page in ["drag-native", "drag-pointer"] {
            s.ok(&["goto", &format!("{}/{page}", site.url)]);
            s.ok(&["eval", "document.getElementById('b').style.marginBottom = '3000px'"]);
            let r = s.ok(&["drag", "#a", "#c"]);
            assert_eq!(r["drag"]["path"], "drag", "{tag} {page}: {r}");
            assert_eq!(r["drag"]["dropped"], true, "{tag} {page}: {r}");
            let log = s.text(&["get", "text", "--selector", "#log"]);
            assert!(log.contains("drop:a"), "{tag} {page}: {log}");
            assert_eq!(list_order(&s), "bca", "{tag} {page}");
        }
    }
}

/// A ref whose element left the page fails at once and says so; a bare
/// `draggable=true` element gets a ref without any handler on it.
#[test]
fn a_stale_ref_fails_at_once_and_a_draggable_has_a_ref() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("stale-ref", &[]);
    s.ok(&["goto", &format!("{}/stale-ref", site.url)]);
    let ax = s.text(&["ax"]);
    assert!(ax.lines().any(|l| l.contains("Handle") && l.contains("[ref=")), "{ax}");
    let go = ax.lines().find(|l| l.contains("button \"Go\"")).unwrap();
    let r = go.split("[ref=").nth(1).unwrap().split(']').next().unwrap().to_string();
    s.ok(&["eval", "document.getElementById('b').remove()"]);
    let t = std::time::Instant::now();
    let e = s.fails(&["click", &r]);
    assert!(e.contains("stale") && !e.contains("--force"), "{e}");
    assert!(t.elapsed() < std::time::Duration::from_millis(2000), "took {:?}", t.elapsed());
}

/// `--from-at`/`--to-at` press and drop at a point of the element (from its
/// top-left corner, as Playwright's sourcePosition/targetPosition); the
/// default is its center.
#[test]
fn drag_presses_and_drops_where_asked() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("dragat", &[]);
    for (args, drop) in [
        (&[][..], "100,50"),
        (&["--to-at", "20,5"][..], "20,5"),
        (&["--to-at", "150,90", "--from-at", "10,10"][..], "150,90"),
    ] {
        s.ok(&["goto", &format!("{}/drag-position", site.url)]);
        let mut cmd = vec!["drag", "#src", "#dst"];
        cmd.extend_from_slice(args);
        let r = s.ok(&cmd);
        assert_eq!(r["drag"]["dropped"], true, "{args:?}: {r}");
        let got = s.text(&["get", "text", "--selector", "#log"]);
        let (gx, gy) =
            got.split_once(',').map(|(x, y)| (x.parse::<f64>().unwrap(), y.parse::<f64>().unwrap())).unwrap();
        let (wx, wy) =
            drop.split_once(',').map(|(x, y)| (x.parse::<f64>().unwrap(), y.parse::<f64>().unwrap())).unwrap();
        assert!((gx - wx).abs() <= 1.0 && (gy - wy).abs() <= 1.0, "{args:?}: dropped at {got}, wanted {drop}");
    }
    let press = s.text(&["eval", "document.getElementById('log').dataset.press"]);
    assert_eq!(press, "10,10", "--from-at presses there");
}

/// With no drag to start, `drag` is a held mouse: the knob follows it.
#[test]
fn drag_without_a_drag_is_a_held_mouse() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    for (tag, mobile) in [("desk", false), ("mob", true)] {
        let s = Session::start(&format!("drags-{tag}"), &[]);
        if mobile {
            s.ok(&["set", "device", "iPhone 15"]);
        }
        s.ok(&["goto", &format!("{}/drag-slider", site.url)]);
        let r = s.ok(&["drag", "#knob", "#track"]);
        assert_eq!(r["drag"]["path"], "mouse", "{tag}: {r}");
        assert!(r["drag"]["from"]["x"].is_number() && r["drag"]["to"]["x"].is_number(), "{tag}: {r}");
        let out: f64 = s.text(&["get", "text", "--selector", "#out"]).parse().unwrap();
        assert!(out > 100.0, "{tag}: knob at {out}");
    }
}

/// How long `args` takes to run on the session, in ms (the CLI start-up is in
/// it for every sample alike).
/// Held by the tests that time an op against another, and by the ones that
/// load the machine with large-page reads: run side by side, the second
/// kind slowed the first past its margin on two-core runners.
static TIMED: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timed_alone() -> std::sync::MutexGuard<'static, ()> {
    TIMED.lock().unwrap_or_else(|e| e.into_inner())
}

fn timed(s: &Session, args: &[&str]) -> (f64, Value) {
    let t = std::time::Instant::now();
    let r = s.ok(args);
    (t.elapsed().as_secs_f64() * 1000.0, r)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// A long poll (a request held open, reconnected) and irregular analytics
/// beacons are background traffic: the first snapshot waits at most about
/// a second for the held request, and later ones don't wait on either.
#[test]
fn long_polls_and_beacons_dont_hold_snapshots() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("bg", &[]);
    s.ok(&["goto", &format!("{}/long-poll?hold=2500", site.url)]);
    let (first, _) = timed(&s, &["ax"]);
    assert!(first < 2000.0, "first ax on a long-poll page took {first} ms (the cap is 3 s)");
    let (second, _) = timed(&s, &["ax"]);
    assert!(second < 700.0, "second ax took {second} ms");
    // After the reconnect: the same URL is a long poll, not data.
    s.ok(&["wait", "--text", "reconnected 1"]);
    let after: Vec<f64> = (0..3).map(|_| timed(&s, &["ax"]).0).collect();
    assert!(median(after.clone()) < 700.0, "ax after a reconnect took {after:?} ms");

    // Right after load: the first beacon answered 204, so it held nothing
    // past its own answer (the faster of two loads: a loaded runner's
    // fixture server can be slow to answer one).
    let first = (0..2)
        .map(|_| {
            s.ok(&["goto", &format!("{}/beacons", site.url)]);
            timed(&s, &["ax"]).0
        })
        .fold(f64::MAX, f64::min);
    assert!(first < 450.0, "first ax on a page sending beacons took {first} ms (the quiet window is 500)");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let beacons: Vec<f64> = (0..5).map(|_| timed(&s, &["ax"]).0).collect();
    assert!(median(beacons.clone()) < 700.0, "ax on a page sending beacons took {beacons:?} ms");
}

/// A snapshot waits for the network to go quiet so an SPA's "Loading..." list
/// has landed, but a request repeated at a steady interval is a poll and
/// doesn't hold it; `--no-wait` skips the wait.
#[test]
fn a_snapshot_does_not_wait_on_polls_and_no_wait_skips_the_wait() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("quiet", &[]);

    // One fetch the server holds for 400 ms: the list is there, not "Loading...".
    s.ok(&["goto", &format!("{}/one-fetch", site.url)]);
    let tree = s.text(&["ax"]);
    assert!(tree.contains("Alpha") && !tree.contains("Loading"), "{tree}");
    // --no-wait reads at once: the placeholder (the fetch is still out).
    s.ok(&["goto", &format!("{}/one-fetch", site.url)]);
    let early = s.text(&["ax", "--no-wait"]);
    assert!(early.contains("Loading"), "{early}");

    // A page polling every 300 ms: the wait never holds a snapshot for the
    // 3 s cap. Let the polling settle first (two requests are not yet a poll).
    s.ok(&["goto", &format!("{}/poll", site.url)]);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let waited: Vec<f64> = (0..5).map(|_| timed(&s, &["ax"]).0).collect();
    let skipped: Vec<f64> = (0..5).map(|_| timed(&s, &["ax", "--no-wait"]).0).collect();
    assert!(median(waited.clone()) < 1000.0, "ax on a polling page took {waited:?} ms");
    assert!(median(skipped.clone()) < 1000.0, "ax --no-wait took {skipped:?} ms");
    assert!(s.text(&["ax", "--no-wait"]).contains("polled"));
    s.ok(&["screenshot", "--no-wait", "--path", &std::env::temp_dir().join("nv-quiet.png").display().to_string()]);
}

/// A snapshot after a successful `wait` skips the quiet wait (and says
/// nothing about it), while a snapshot after a click still waits and
/// says how long it waited.
#[test]
fn a_snapshot_after_a_wait_skips_the_quiet_wait_and_reports_it() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("quiet-skip", &[]);
    let has_quiet_note = |r: &Value| {
        r["notes"]
            .as_array()
            .map(|n| n.iter().any(|x| x.as_str().unwrap_or("").contains("network quiet")))
            .unwrap_or(false)
    };

    s.ok(&["goto", &format!("{}/one-fetch", site.url)]);
    s.ok(&["wait", "--text", "Alpha"]);
    // The FIRST snapshot after the wait must skip: a median over several
    // would hide it, since later snapshots are quiet anyway.
    let t = std::time::Instant::now();
    let (ok, r) = s.run(&["ax"]);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(ok, "ax failed: {r}");
    assert!(ms < 300.0, "first ax after wait --text took {ms:.0} ms");
    assert!(!has_quiet_note(&r), "a skipped wait should report nothing: {r}");
    assert!(r["result"].as_str().unwrap_or("").contains("Alpha"));

    // A click is not a wait: the snapshot after it waits, and says so.
    s.ok(&["goto", &format!("{}/click-fetch", site.url)]);
    s.ok(&["click", "--role", "button", "--name", "Go"]);
    let (_, r) = s.run(&["ax"]);
    assert!(has_quiet_note(&r), "a snapshot that waited should say so: {r}");
    assert!(r["result"].as_str().unwrap_or("").contains("Alpha"));
}

/// A click on a link or a submit button waits for the page it loads only
/// when the page lets it load one: a router that cancels the link's click,
/// a script that handles the submit, or a form that fails validation leave
/// nothing to wait for, so such a click costs what a plain click costs, not
/// a navigation's grace on top. A script that submits the form itself is a
/// navigation and is waited for. Nothing navigera watched is left behind.
#[test]
fn a_click_waits_for_a_navigation_only_when_the_page_plans_one() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("nav-intent", &[]);
    let page = format!("{}/nav-intent", site.url);
    for name in ["Real submit", "Script submit", "Real link"] {
        s.ok(&["goto", &page]);
        let r = s.ok(&["click", "--text", name]);
        assert!(r["url"].as_str().unwrap_or_default().contains("/posted"), "{name}: {r}");
    }
    for (name, out, path) in [
        ("Handled submit", "handled", "/nav-intent"),
        ("Routed link", "routed", "/routed"),
        ("Invalid submit", "idle", "/nav-intent"),
    ] {
        s.ok(&["goto", &page]);
        s.ok(&["click", "--text", name]);
        assert_eq!(s.ok(&["eval", "location.pathname"]), path, "{name}");
        let shown = s.ok(&["text", "--selector", "#out"]);
        assert!(shown.as_str().unwrap_or_default().starts_with(out), "{name}: {shown}");
        let clean =
            "window.__nvNav === undefined && HTMLFormElement.prototype.submit.toString().includes('[native code]')";
        assert_eq!(s.ok(&["eval", clean]), true, "{name}: watchers left behind");
    }
    let best = |name: &str| {
        (0..3)
            .map(|_| {
                s.ok(&["goto", &page]);
                timed(&s, &["click", "--text", name]).0
            })
            .fold(f64::MAX, f64::min)
    };
    let (plain, handled) = (best("Plain"), best("Handled submit"));
    assert!(
        handled < plain + 100.0,
        "a handled submit waited for a navigation: {handled:.0} ms against {plain:.0} ms for a plain click"
    );
}

fn assert_chain_reports_its_end(page: &str) {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start(page.trim_start_matches('/'), &[]);
    let compact = |args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .arg("--session")
            .arg(&s.name)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for _ in 0..3 {
        s.ok(&["goto", &format!("{}{page}", site.url)]);
        let tree = compact(&["ax"]);
        let reference = tree
            .lines()
            .find(|l| l.contains("button \"Load\""))
            .and_then(|l| l.split("[ref=").nth(1))
            .map(|r| r.trim_end_matches(']').to_string())
            .unwrap_or_else(|| panic!("no Load ref in {tree}"));
        std::thread::sleep(std::time::Duration::from_millis(600));
        let out = compact(&["click", &reference]);
        assert!(out.contains("chain complete"), "{out}");
        assert!(!out.contains("phase one"), "{out}");
    }
}

/// A click whose response starts a second fetch at once reports what the
/// second fetch rendered: the after-action settle never returns between
/// states of a chain.
#[test]
fn an_action_reports_the_end_of_a_chained_fetch() {
    assert_chain_reports_its_end("/action-chain");
}

/// The same with the second fetch deferred by a timer inside the quiet
/// window (80 ms): the settle proof waits for short timers the page
/// scheduled, as the plain window did.
#[test]
fn an_action_reports_the_end_of_a_fetch_chained_by_a_timer() {
    assert_chain_reports_its_end("/action-chain-timer");
}

/// What an action changed is read after the requests it started, also one
/// a timer starts a turn later with nothing shown first (typing that asks
/// for suggestions): the report has the answer, not the page as it was. The
/// page has been quiet for longer than the quiet window (an agent that
/// thought before typing), so the quiet wait alone would read it at once.
#[test]
fn an_action_reports_what_a_request_it_started_a_turn_later_changed() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("turn-fetch", &[]);
    let compact = |args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .arg("--session")
            .arg(&s.name)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for word in ["ab", "cd", "ef"] {
        s.ok(&["goto", &format!("{}/nav-intent", site.url)]);
        compact(&["ax"]);
        std::thread::sleep(std::time::Duration::from_millis(600));
        let out = compact(&["fill", "--selector", "#q", word]);
        assert!(out.contains(&format!("suggested {word}")), "{out}");
    }
    // A placeholder the click put up for work it starts 30 ms later is not
    // the report: the page's next change is waited for.
    for _ in 0..3 {
        s.ok(&["goto", &format!("{}/nav-intent", site.url)]);
        compact(&["ax"]);
        std::thread::sleep(std::time::Duration::from_millis(600));
        let out = compact(&["click", "--text", "Save later"]);
        assert!(out.contains("saved") && !out.contains("Saving"), "{out}");
    }
    // An action that starts nothing isn't held for a request that never
    // comes: saying what it changed costs a read of the page, not a fixed
    // wait (it was 50 ms). The two kinds of fill take turns, so a burst of
    // load from the tests beside this one slows both, not just the ones
    // timed last (CI: 56 ms against 11 ms, with each kind timed in a block).
    let fill = |report: bool, i: usize| {
        s.ok(&["goto", &format!("{}/nav-intent", site.url)]);
        compact(&["ax"]);
        let value = format!("v{i}");
        let t = std::time::Instant::now();
        if report {
            compact(&["fill", "--selector", "[name=x]", &value]);
        } else {
            s.ok(&["fill", "--selector", "[name=x]", &value]);
        }
        t.elapsed().as_secs_f64() * 1000.0
    };
    let (mut plain, mut reported) = (f64::MAX, f64::MAX);
    for i in 0..5 {
        plain = plain.min(fill(false, i));
        reported = reported.min(fill(true, i));
    }
    assert!(reported < plain + 40.0, "saying what a fill changed took {reported:.0} ms against {plain:.0} ms");
}

/// On a large page, text entered into a field that changes nothing else
/// is reported without reading the page again: the journal armed with the
/// last read proves it. The report is what a read after the entry says,
/// entry by entry and for the click after them (the typed text stays out
/// of it), whichever way the page changed: its DOM, a style that follows
/// focus, a `:has()` rule. A check, a radio switch and a choice are their
/// controls' own lines, rewritten; a state a script set after the read is
/// reported with the next entry. Compared against a session that reads
/// the page after every entry (`NAVIGERA_CHANGE_JOURNAL=0`), and timed
/// against it. A plain `ax` after an entry the journal answered is
/// patched (no `DOMSnapshot.captureSnapshot` in the session's CDP trace)
/// and says what the other session's says.
#[test]
fn a_text_entry_that_changes_nothing_else_skips_reading_the_page_again() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let page = format!("{}/journal", site.url);
    let trace_of =
        |on: &str| std::env::temp_dir().join(format!("nv-interactions-journal{on}-{}.jsonl", std::process::id()));
    let sessions: Vec<(String, &str)> = ["1", "0"]
        .iter()
        .map(|on| {
            let name = std::env::temp_dir()
                .join(format!("nv-interactions-journal{on}-{}.sock", std::process::id()))
                .display()
                .to_string();
            let _ = std::fs::remove_file(trace_of(on));
            let out = Command::new(tool_exe())
                .env("NAVIGERA_CHANGE_JOURNAL", on)
                .env("NAVIGERA_CDP_TRACE", trace_of(on))
                .args(["--session", &name, "start"])
                .stdin(Stdio::null())
                .output()
                .expect("run navigera");
            assert!(out.status.success(), "start failed: {}", String::from_utf8_lossy(&out.stdout));
            (name, *on)
        })
        .collect();
    let compact = |name: &str, args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .args(["--session", name])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let steps: [&[&str]; 15] = [
        &["fill", "--selector", "#name", "Ada Lovelace"],
        &["fill", "--selector", "#note", "first note"],
        &["fill", "--selector", "#count", "abc"],
        &["fill", "--selector", "#tipped", "Grace"],
        &["fill", "--selector", "#code", "X1"],
        &["fill", "--selector", "#name", "Ada"],
        &["fill", "--selector", "#restyle", "on"],
        &["fill", "--selector", "#imported", "x"],
        &["click", "--selector", "#go"],
        &["check", "--selector", "#agree"],
        &["select", "--selector", "#size", "Large"],
        &["check", "--selector", "#air"],
        &["eval", "document.getElementById('later').checked = true"],
        &["fill", "--selector", "#name", "Grace Hopper"],
        &["click", "--selector", "#go"],
    ];
    let mut reports: Vec<Vec<String>> = Vec::new();
    for (name, _) in &sessions {
        compact(name, &["goto", &page]);
        compact(name, &["ax"]);
        let mut r: Vec<String> = steps.iter().map(|a| compact(name, a)).collect();
        // A closed shadow root, which the page's scripts (and so the
        // journal) can't reach: its change is reported all the same.
        compact(name, &["goto", &format!("{page}?closed")]);
        compact(name, &["ax"]);
        r.push(compact(name, &["fill", "--selector", "#inclosed", "z"]));
        reports.push(r);
    }
    // Refs are node ids: each browser numbers its own.
    let unref = |r: &Vec<String>| -> Vec<String> {
        r.iter()
            .map(|l| {
                l.split("ref=")
                    .enumerate()
                    .map(|(i, p)| if i == 0 { p } else { p.trim_start_matches(|c: char| c.is_ascii_digit()) })
                    .collect()
            })
            .collect()
    };
    assert_eq!(unref(&reports[0]), unref(&reports[1]), "the journal changed what an entry reported");
    let r = &reports[0];
    assert_eq!(r[0], "ok", "{r:?}");
    assert_eq!(r[1], "ok", "{r:?}");
    assert!(r[2].contains("+ text: 3 chars"), "a live count is the entry's change: {r:?}");
    assert!(r[3].contains("Use your full name"), "a style that follows focus: {r:?}");
    assert!(r[4].contains("- text: Type a code"), "a :has() rule: {r:?}");
    // "Ada" after "Ada Lovelace": the field showed it already, so it changed.
    assert!(r[5].contains("~ textbox \"Name\""), "{r:?}");
    assert!(r[6].contains("+ text: Restyled by a rule"), "a style rule a script rewrote: {r:?}");
    assert!(r[7].contains("Imported tip"), "a style from an @imported sheet: {r:?}");
    assert!(r[8].contains("clicked 1") && !r[8].contains("Lovelace") && !r[8].contains("first note"), "{r:?}");
    assert!(r[9].contains("~ checkbox \"Agree\" [checked, ref="), "{r:?}");
    assert!(r[10].contains("value=\"Large\"") && r[10].contains("\"Large (selected)\""), "{r:?}");
    assert!(r[11].contains("~ radio \"Ground\" [ref=") && r[11].contains("~ radio \"Air\" [checked, ref="), "{r:?}");
    assert!(r[13].contains("~ checkbox \"Later\" [checked, ref="), "a state a script set after the read: {r:?}");
    assert!(
        r[14].contains("clicked 2") && !["Agree", "Large", "Air", "Later"].iter().any(|s| r[14].contains(s)),
        "{r:?}"
    );
    assert!(r[15].contains("closed changed"), "inside a closed shadow root: {r:?}");
    let best = |name: &str| {
        (0..3)
            .map(|i| {
                compact(name, &["goto", &page]);
                compact(name, &["ax"]);
                let t = std::time::Instant::now();
                assert_eq!(compact(name, &["fill", "--selector", "#name", &format!("v{i}")]), "ok");
                t.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::MAX, f64::min)
    };
    let (journal, read) = (best(&sessions[0].0), best(&sessions[1].0));
    assert!(
        journal < read * 0.75,
        "an entry with the journal took {journal:.0} ms, reading the page again {read:.0} ms"
    );
    // A read after each entry the journal answered.
    let captures = |on: &str| {
        std::fs::read_to_string(trace_of(on))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"DOMSnapshot.captureSnapshot\""))
            .count()
    };
    let entries: [&[&str]; 4] = [
        &["fill", "--selector", "#name", "Ada Lovelace"],
        &["check", "--selector", "#agree"],
        &["select", "--selector", "#size", "Large"],
        &["fill", "--selector", "#note", "a note"],
    ];
    let mut reads: Vec<Vec<String>> = Vec::new();
    let mut caps = Vec::new();
    for (name, on) in &sessions {
        compact(name, &["goto", &page]);
        compact(name, &["ax"]);
        let before = captures(on);
        reads.push(
            entries
                .iter()
                .flat_map(|a| {
                    let report = compact(name, a);
                    [report, compact(name, &["ax"])]
                })
                .collect(),
        );
        caps.push(captures(on) - before);
    }
    for (name, _) in &sessions {
        compact(name, &["quit"]);
    }
    if let Some(ci) = std::env::var_os("NAVIGERA_CDP_TRACE") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(ci) {
            for on in ["1", "0"] {
                let _ = f.write_all(std::fs::read_to_string(trace_of(on)).unwrap_or_default().as_bytes());
            }
        }
    }
    for on in ["1", "0"] {
        let _ = std::fs::remove_file(trace_of(on));
    }
    assert_eq!(unref(&reads[0]), unref(&reads[1]), "a read after an entry the journal answered differs");
    assert!(reads[0].iter().step_by(2).all(|r| !r.is_empty()), "{:?}", reads[0]);
    assert_eq!(caps[0], 0, "reads after entries the journal answered captured the page {} times", caps[0]);
    assert_eq!(caps[1], 8, "the session without the journal captured the page {} times", caps[1]);
}

/// On a large page, the read that says what a click changed patches the
/// last read's snapshot with the subtrees the change journal names, read
/// from the page, instead of capturing the page whole. Whatever the click
/// changed (a cell, a row added or removed, a menu a sibling rule shows, a
/// dialog, a popover, a list drawn again, a listener, a value a script set,
/// a fade, a numbered list), each report and the look after them are what
/// a session that captures the page every time says
/// (`NAVIGERA_DELTA_READ=0`), and the closed `<details>` stays closed
/// (reading the style of content a display lock skips would lay it out for
/// the next capture). Opened and closed again, its body is reported shown
/// and then gone in both sessions, though Chrome keeps the boxes it had.
/// No report lists rows that only moved across the cut of the 2,000-line
/// tree. A plain `ax` after each click (what an agent reads next) is
/// patched too, and says what that session says. While clicks on that
/// page, each with an `ax` after it, are timed, the patching session sends
/// no `DOMSnapshot.captureSnapshot` and the other sends one per read (the
/// CDP trace of each session); the times are printed, not asserted, as a
/// shared runner's debug build can't tell them apart reliably.
#[test]
fn a_click_on_a_large_page_reads_only_what_it_changed() {
    let _alone = timed_alone();
    let (true, Some(site)) = (chrome(), site()) else { return };
    let page = format!("{}/clicks", site.url);
    let trace_of =
        |on: &str| std::env::temp_dir().join(format!("nv-interactions-delta{on}-{}.jsonl", std::process::id()));
    let sessions: Vec<String> = ["1", "0"]
        .iter()
        .map(|on| {
            let name = std::env::temp_dir()
                .join(format!("nv-interactions-delta{on}-{}.sock", std::process::id()))
                .display()
                .to_string();
            let _ = std::fs::remove_file(trace_of(on));
            let out = Command::new(tool_exe())
                .env("NAVIGERA_DELTA_READ", on)
                .env("NAVIGERA_CDP_TRACE", trace_of(on))
                .args(["--session", &name, "start"])
                .stdin(Stdio::null())
                .output()
                .expect("run navigera");
            assert!(out.status.success(), "start failed: {}", String::from_utf8_lossy(&out.stdout));
            name
        })
        .collect();
    let compact = |name: &str, args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .args(["--session", name])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let buttons = [
        "#edit", "#add", "#remove", "#menu", "#show", "#dlg", "#pop", "#redraw", "#wire", "#val", "#fade", "#ol",
        "#edit", "#det", "#det",
    ];
    let mut reports: Vec<Vec<String>> = Vec::new();
    let mut rereads: Vec<Vec<String>> = Vec::new();
    for name in &sessions {
        compact(name, &["goto", &page]);
        compact(name, &["ax"]);
        let mut again = Vec::new();
        let mut r: Vec<String> = buttons
            .iter()
            .map(|b| {
                let report = compact(name, &["click", "--selector", b]);
                again.push(compact(name, &["ax"]));
                report
            })
            .collect();
        r.push(compact(name, &["ax"]));
        reports.push(r);
        rereads.push(again);
    }
    // Refs are node ids: each browser numbers its own.
    let unref = |r: &Vec<String>| -> Vec<String> {
        r.iter()
            .map(|l| {
                l.split("ref=")
                    .enumerate()
                    .map(|(i, p)| if i == 0 { p } else { p.trim_start_matches(|c: char| c.is_ascii_digit()) })
                    .collect()
            })
            .collect()
    };
    // A click that changed nothing says how long it watched.
    let unms = |r: Vec<String>| -> Vec<String> {
        r.iter()
            .map(|l| {
                let parts: Vec<&str> = l.split(" ms").collect();
                let last = parts.len() - 1;
                parts
                    .iter()
                    .enumerate()
                    .map(|(i, p)| if i < last { p.trim_end_matches(|c: char| c.is_ascii_digit()) } else { p })
                    .collect::<Vec<_>>()
                    .join(" ms")
            })
            .collect()
    };
    assert_eq!(
        unms(unref(&reports[0])),
        unms(unref(&reports[1])),
        "patching the snapshot changed what a click reported"
    );
    assert_eq!(
        unms(unref(&rereads[0])),
        unms(unref(&rereads[1])),
        "patching the snapshot changed what a read after a click said"
    );
    let r = &reports[0];
    assert!(r[0].contains("edited 1"), "{r:?}");
    assert!(r[1].contains("Inserted"), "a row added: {r:?}");
    assert!(r[2].contains("- cell \"SKU-00004\""), "a row removed: {r:?}");
    assert!(r[3].contains("Alpha"), "a menu a sibling rule shows: {r:?}");
    assert!(r[4].contains("Hidden section"), "{r:?}");
    assert!(r[5].contains("Dialog text"), "{r:?}");
    assert!(r[6].contains("Popover body"), "{r:?}");
    assert!(r[7].contains("Item 0"), "a list drawn again: {r:?}");
    assert!(r[9].contains("set by script"), "a value a script set: {r:?}");
    assert!(r[11].contains("zeroth"), "{r:?}");
    assert!(r[12].contains("edited 2"), "{r:?}");
    assert!(r[..13].iter().all(|l| !l.contains("Details body")), "a closed <details> showed its body: {r:?}");
    assert!(r[13].contains("+ paragraph: Details body"), "a <details> opened: {r:?}");
    assert!(r[14].contains("- paragraph: Details body"), "a <details> closed again: {r:?}");
    assert!(!r[15].contains("Details body"), "a <details> closed again showed its body: {r:?}");
    // The changes are all near the top: a row past the first hundred in a
    // report only moved across the cut.
    let past_top = |line: &str| {
        ["SKU-", "\"Item "].iter().any(|tag| {
            line.split(tag).skip(1).any(|p| {
                let n: String = p.chars().take_while(char::is_ascii_digit).collect();
                n.parse::<u32>().is_ok_and(|n| n >= 100)
            })
        })
    };
    assert!(
        r[..15].iter().all(|l| !l.lines().any(past_top)),
        "a report listed rows that only moved across the cut: {r:?}"
    );
    let captures = |on: &str| {
        std::fs::read_to_string(trace_of(on))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"DOMSnapshot.captureSnapshot\""))
            .count()
    };
    // Best of 3 clicks, each with a read after it, and the full captures
    // sent while they ran.
    let best = |name: &str, on: &str| {
        compact(name, &["goto", &page]);
        compact(name, &["ax"]);
        let before = captures(on);
        let ms = (0..3)
            .map(|_| {
                let t = std::time::Instant::now();
                assert!(compact(name, &["click", "--selector", "#edit"]).contains("edited"));
                assert!(compact(name, &["ax"]).contains("edited"));
                t.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::MAX, f64::min);
        (ms, captures(on) - before)
    };
    let ((patched, patched_caps), (read, read_caps)) = (best(&sessions[0], "1"), best(&sessions[1], "0"));
    eprintln!("a click's report and a read: {patched:.0} ms patched, {read:.0} ms captured whole");
    for name in &sessions {
        compact(name, &["quit"]);
    }
    // Keep CI's protocol check seeing what these sessions sent.
    if let Some(ci) = std::env::var_os("NAVIGERA_CDP_TRACE") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(ci) {
            for on in ["1", "0"] {
                let _ = f.write_all(std::fs::read_to_string(trace_of(on)).unwrap_or_default().as_bytes());
            }
        }
    }
    for on in ["1", "0"] {
        let _ = std::fs::remove_file(trace_of(on));
    }
    assert_eq!(
        read_caps, 6,
        "the session capturing every read captured the page {read_caps} times for 3 clicks and reads"
    );
    assert_eq!(patched_caps, 0, "patched reads captured the page whole {patched_caps} times in 3 clicks and reads");
}

/// A read on a page whose style rules far outnumber its elements doesn't
/// walk them for the next read (walking costs more than capturing that
/// page): the plain `ax` after it captures again. The first action's prime
/// walks them, as it did before reads primed, and from then on the read
/// after an action and the plain `ax` after that are patched (no
/// `DOMSnapshot.captureSnapshot` in the session's CDP trace).
#[test]
fn a_read_leaves_walking_a_large_style_sheet_to_the_first_action() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let trace = std::env::temp_dir().join(format!("nv-interactions-rules-{}.jsonl", std::process::id()));
    let name =
        std::env::temp_dir().join(format!("nv-interactions-rules-{}.sock", std::process::id())).display().to_string();
    let _ = std::fs::remove_file(&trace);
    let out = Command::new(tool_exe())
        .env("NAVIGERA_CDP_TRACE", &trace)
        .env_remove("NAVIGERA_REST_PRIME")
        .args(["--session", &name, "start"])
        .stdin(Stdio::null())
        .output()
        .expect("run navigera");
    assert!(out.status.success(), "start failed: {}", String::from_utf8_lossy(&out.stdout));
    let run = |args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .args(["--session", &name])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let captures = || {
        std::fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"DOMSnapshot.captureSnapshot\""))
            .count()
    };
    run(&["goto", &format!("{}/many-rules", site.url)]);
    assert!(run(&["ax"]).contains("Row 0"));
    let first = captures();
    assert!(run(&["ax"]).contains("Row 0"));
    assert_eq!(captures(), first + 1, "the read after a read that walked no rules captures the page");
    let report = run(&["click", "--selector", "#add"]);
    assert!(report.contains("Added row"), "{report}");
    assert!(run(&["ax"]).contains("Added row"));
    assert_eq!(captures(), first + 1, "after the action walked the rules, its read and the next are patched");
    run(&["quit"]);
    let _ = std::fs::remove_file(&trace);
}

/// A client that pauses between commands (an agent reading what `goto`
/// said) has the large page read ahead while it pauses: its first `ax`
/// captures nothing and still shows what changed after `goto`, even when
/// a setup sent `start` and a first `goto` at once. A client that sends
/// its next command at once (a script) gets no read ahead.
#[test]
fn the_first_read_after_a_paused_goto_was_read_ahead() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let trace = std::env::temp_dir().join(format!("nv-interactions-ahead-{}.jsonl", std::process::id()));
    let name =
        std::env::temp_dir().join(format!("nv-interactions-ahead-{}.sock", std::process::id())).display().to_string();
    let _ = std::fs::remove_file(&trace);
    let out = Command::new(tool_exe())
        .env("NAVIGERA_CDP_TRACE", &trace)
        .env_remove("NAVIGERA_PRE_READ")
        .env_remove("NAVIGERA_REST_PRIME")
        .args(["--session", &name, "start"])
        .stdin(Stdio::null())
        .output()
        .expect("run navigera");
    assert!(out.status.success(), "start failed: {}", String::from_utf8_lossy(&out.stdout));
    let run = |args: &[&str]| {
        let out = Command::new(tool_exe())
            .env_remove("NAVIGERA_OUTPUT")
            .args(["--session", &name])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let captures = || {
        std::fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"DOMSnapshot.captureSnapshot\""))
            .count()
    };
    let pause = || std::thread::sleep(std::time::Duration::from_millis(1500));
    let page = format!("{}/clicks", site.url);
    // A setup's goto, sent at once after start: not read ahead itself,
    // and it doesn't stop the paused goto that follows.
    run(&["goto", &page]);
    pause();
    run(&["goto", &page]);
    pause();
    let before = captures();
    assert!(before >= 1, "the page was read ahead while the client paused");
    // A change between the read ahead and the read is shown.
    run(&["eval", "document.querySelector('#r5 td:nth-child(2)').textContent = 'changed after goto'"]);
    pause();
    let tree = run(&["ax"]);
    assert!(tree.contains("changed after goto") && tree.contains("SKU-00000"), "{tree}");
    assert_eq!(captures(), before, "the first read patched what was read ahead");
    // Commands sent at once: no read ahead, the read captures.
    for _ in 0..3 {
        run(&["title"]);
    }
    run(&["goto", &page]);
    let before = captures();
    assert!(run(&["ax"]).contains("SKU-00000"));
    assert_eq!(captures(), before + 1, "a script's read captures for itself");
    run(&["quit"]);
    let _ = std::fs::remove_file(&trace);
}

/// After a click whose fetch has started, the snapshot waits for that fetch
/// and a short settle, not the whole quiet window: the list is there, well
/// before the 200 ms fetch plus 500 ms of quiet it used to take. A window
/// the session set holds after actions too.
#[test]
fn a_snapshot_after_a_click_that_fetches_waits_for_the_fetch_not_the_window() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("after-click", &[]);
    let mut times = Vec::new();
    for _ in 0..3 {
        s.ok(&["goto", &format!("{}/click-fetch", site.url)]);
        s.ok(&["click", "--role", "button", "--name", "Go"]);
        let (ms, r) = timed(&s, &["ax"]);
        let tree = r.as_str().unwrap_or("");
        assert!(tree.contains("Alpha") && !tree.contains("Loading"), "{tree}");
        times.push(ms);
    }
    assert!(median(times.clone()) < 650.0, "ax after a click that fetches took {times:?} ms");

    s.ok(&["set", "network-quiet", "800"]);
    s.ok(&["goto", &format!("{}/click-fetch", site.url)]);
    s.ok(&["click", "--role", "button", "--name", "Go"]);
    let (ms, r) = timed(&s, &["ax"]);
    assert!(r.as_str().unwrap_or("").contains("Alpha"), "{r}");
    assert!(ms >= 800.0, "a chosen window holds after a click: {ms} ms");

    // `eval --main` is for an Electron app's main process.
    let (ok, r) = s.run(&["eval", "--main", "1"]);
    assert!(!ok && r.to_string().contains("--electron"), "{r}");
}

/// The default quiet window reads a list that arrives in two fetches with
/// a 400 ms gap between them; a 250 ms window ends in the gap and reads
/// only the first half. That is why the default stays 500 ms and 250 ms
/// is opt-in per session.
#[test]
fn a_short_quiet_window_reads_a_chained_list_early() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("quiet-window", &[]);

    s.ok(&["goto", &format!("{}/two-fetch", site.url)]);
    let tree = s.text(&["ax"]);
    assert!(tree.contains("Alpha") && tree.contains("Beta"), "{tree}");

    s.ok(&["set", "network-quiet", "250"]);
    s.ok(&["goto", &format!("{}/two-fetch", site.url)]);
    let tree = s.text(&["ax"]);
    assert!(tree.contains("Alpha") && !tree.contains("Beta"), "{tree}");

    s.ok(&["set", "network-quiet", "500"]);
    s.ok(&["goto", &format!("{}/two-fetch", site.url)]);
    let tree = s.text(&["ax"]);
    assert!(tree.contains("Alpha") && tree.contains("Beta"), "{tree}");
}

/// `check` on a controlled checkbox (the click's default is cancelled and the
/// app commits the state later, as a React onChange re-render does) succeeds
/// once the state lands, not only when it is already there as the click
/// returns; one that never commits still fails, inside the element's budget.
#[test]
fn check_waits_for_a_late_commit() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("latecheck", &[]);
    s.ok(&["goto", &format!("{}/hidden-layers", site.url)]);
    let page = |delay: &str| {
        format!(
            "document.body.innerHTML = '<label><input type=checkbox id=c> Forecast</label>'; \
             document.getElementById('c').addEventListener('click', (e) => {{ e.preventDefault(); {delay} }}); 1"
        )
    };
    s.ok(&["eval", &page("setTimeout(() => { document.getElementById('c').checked = true; }, 300);")]);
    assert_eq!(s.ok(&["check", "--selector", "#c"])["changed"], true);
    assert_eq!(s.ok(&["eval", "document.getElementById('c').checked"]), true);
    s.ok(&["eval", &page("")]);
    let error = s.fails(&["check", "--selector", "#c", "--timeout-ms", "1000"]);
    assert!(error.contains("did not check it"), "{error}");
}

/// A page short enough for the whole tree to be read in one call (about as
/// long as the limit) still stops at the limit, and its note gives no count.
#[test]
fn ax_with_a_limit_on_a_mid_size_page_says_more_lines_without_a_count() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("midtable", &[]);
    s.ok(&["goto", &format!("{}/honesty", site.url)]);
    s.ok(&["eval", "(() => { let h = '<ul>'; for (let i = 0; i < 200; i++) h += '<li>Item ' + i + ' <button onclick=\"out(' + i + ')\">Pick ' + i + '</button></li>'; document.body.insertAdjacentHTML('beforeend', h + '</ul>'); return 1; })()"]);
    let tree = s.text(&["ax", "--limit", "300"]);
    assert!(tree.contains("Pick 0") && !tree.contains("Pick 199"), "{tree}");
    let last = tree.lines().last().unwrap();
    assert!(last.starts_with("… more lines"), "{last}");
    assert!(tree.lines().count() <= 302, "{} lines", tree.lines().count());
    s.ok(&["click", "--role", "button", "--name", "Pick 7"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "7");
}

/// A snapshot of a big table with a limit prints only the first rows, says it
/// cut the rest, and its refs work.
#[test]
fn ax_with_a_limit_on_a_big_table_stops_at_the_limit() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("bigtable", &[]);
    s.ok(&["goto", &format!("{}/big-table", site.url)]);
    let tree = s.text(&["ax", "--limit", "30"]);
    assert!(tree.contains("Edit 0") && tree.contains("more lines"), "{tree}");
    assert!(!tree.contains("Edit 1999"), "the limit cut nothing");
    assert!(tree.lines().count() <= 32, "{} lines", tree.lines().count());
    s.ok(&["click", "--role", "button", "--name", "Edit 7"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#out"]), "edit 7");
    let r = s.ok(&["click", "--selector", "tr:nth-child(3) button"]);
    assert!(r.is_object(), "{r}");
}

/// Text in a page of `<div>`s shares lines (a line per `<div>` made such a
/// page over twice the size), and a cut says how much it left out and the
/// ref that reads all of it.
#[test]
fn text_shares_lines_and_a_cut_says_how_to_read_the_rest() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("text", &[]);
    s.ok(&["goto", &format!("{}/div-cells", site.url)]);
    let tree = s.text(&["ax", "--limit", "0"]);
    let text_lines = tree.lines().filter(|l| l.starts_with("- text: ")).count();
    assert!(text_lines < 80, "3,000 cells took {text_lines} lines:\n{tree}");
    assert!(tree.contains("r0c0 r0c1") && tree.contains("r299c9") && tree.contains("- button \"Next\""), "{tree}");
    for i in [0, 150, 299] {
        for j in 0..10 {
            assert!(tree.contains(&format!("r{i}c{j}")), "r{i}c{j} is missing:\n{tree}");
        }
    }

    // Two long texts on a small page: printed in full (the cuts would save
    // less than the call to read them).
    let long = "Lorem ipsum dolor sit amet. ".repeat(40);
    s.ok(&["goto", &format!("{}/long-text", site.url)]);
    let tree = s.text(&["ax"]);
    assert_eq!(tree.matches(long.trim()).count(), 2, "{tree}");
    assert!(!tree.contains("chars: get text") && !tree.contains("cut short"), "{tree}");

    // A 1,500-clause agreement: every clause is there; the long ones are
    // cut, each says how to read the rest, and a line at the top says it
    // once. `--full-text` prints every word.
    s.ok(&["goto", &format!("{}/legal", site.url)]);
    let tree = s.text(&["ax"]);
    let second = tree.lines().nth(1).unwrap_or("");
    assert!(second.starts_with("… 150 texts cut short here (") && second.contains("ax --full-text"), "{second}");
    assert_eq!(tree.matches("The Supplier shall keep obligation").count(), 1500);
    let refs: Vec<&str> = tree.match_indices("chars: get text ").map(|(at, m)| &tree[at + m.len()..]).collect();
    assert_eq!(refs.len(), 150, "every cut says how to read the rest");
    let id: String = refs[0].chars().take_while(char::is_ascii_digit).collect();
    assert!(s.text(&["get", "text", &id]).ends_with("Penalty schedule 0 applies to any breach not cured in time."));
    let full = s.text(&["ax", "--full-text"]);
    assert_eq!(full.matches("Penalty schedule").count(), 150);
    assert!(!full.contains("cut short") && !full.contains("chars: get text"));
    // `text` with no selector: the whole page as it reads.
    let page = s.text(&["text"]);
    assert!(page.starts_with("Master Services Agreement"), "{}", &page[..80]);
    assert_eq!(page.matches("Penalty schedule").count(), 150);
    assert_eq!(page.matches("The Supplier shall keep obligation").count(), 1500);
}

/// A ref whose element the page drew again (a list redrawn, a button
/// replaced) acts on the same element, found by the role, name and place
/// the snapshot showed, and says so; when the page changed more than that
/// (another row of that name, a new URL) the stale ref still fails.
#[test]
fn a_redrawn_element_is_found_again_by_its_ref() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("rerender", &[]);
    s.ok(&["goto", &format!("{}/rerender", site.url)]);
    let tree = s.text(&["ax"]);
    let refs = |tree: &str, name: &str| -> Vec<String> {
        tree.lines()
            .filter(|l| l.contains(&format!("button \"{name}\"")))
            .filter_map(|l| l.split("ref=").nth(1))
            .map(|r| r.chars().take_while(char::is_ascii_digit).collect())
            .collect()
    };
    let (save, edits) = (refs(&tree, "Save")[0].clone(), refs(&tree, "Edit"));
    assert_eq!(edits.len(), 3, "{tree}");

    s.ok(&["click", "--role", "button", "--name", "Redraw"]);
    let (ok, r) = s.run(&["click", &save]);
    assert!(ok, "{r}");
    let note = r["notes"][0].as_str().unwrap_or("");
    assert!(note.starts_with(&format!("ref {save} was re-rendered: this acted on button \"Save\", now ref ")), "{r}");
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "saved");
    // The second of three same-named buttons stays the second; twice over.
    s.ok(&["click", "--role", "button", "--name", "Redraw"]);
    s.ok(&["click", &edits[1]]);
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "edit beta");
    s.ok(&["click", "--role", "button", "--name", "Redraw"]);
    s.ok(&["click", &edits[1]]);
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "edit beta");

    // A fourth Edit: which one it was is no longer certain.
    let tree = s.text(&["ax"]);
    let edit = refs(&tree, "Edit")[1].clone();
    s.ok(&["click", "--role", "button", "--name", "Add row"]);
    let (ok, r) = s.run(&["click", &edit]);
    assert!(!ok && r.to_string().contains("that ref is stale"), "{r}");
    // Another URL: another view, even with the same buttons.
    let tree = s.text(&["ax"]);
    let save = refs(&tree, "Save")[0].clone();
    s.ok(&["click", "--role", "button", "--name", "Route"]);
    let (ok, r) = s.run(&["click", &save]);
    assert!(!ok && r.to_string().contains("that ref is stale"), "{r}");
}

/// A banner with controls of its own that stays over the button is said
/// after about a second, naming its controls; an overlay over a page that
/// is still fetching is waited for, as long as the element wait.
#[test]
fn a_cover_that_stays_is_said_quickly_and_a_loading_one_is_waited_for() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("covers", &[]);
    s.ok(&["goto", &format!("{}/cookie-banner", site.url)]);
    let t = std::time::Instant::now();
    let err = s.fails(&["click", "--selector", "#buy"]);
    let ms = t.elapsed().as_millis();
    assert!(ms < 2500, "a banner that stays took {ms} ms to report");
    assert!(err.contains("div#consent") && err.contains("\"Accept\", \"Reject\""), "{err}");
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "waiting");
    s.ok(&["click", "--role", "button", "--name", "Accept"]);
    s.ok(&["click", "--selector", "#buy"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "bought");

    s.ok(&["goto", &format!("{}/loading-cover", site.url)]);
    s.ok(&["click", "--selector", "#save"]);
    assert_eq!(s.text(&["get", "text", "--selector", "#status"]), "saved");
}

/// A control with no name or text says what the page marks it with: a
/// tooltip attribute, its icon's class or sprite symbol, its image's file,
/// or the description it points to.
#[test]
fn a_nameless_control_says_what_the_page_marks_it_with() {
    if !chrome() {
        return;
    }
    let s = Session::start("icons", &[]);
    let page = concat!(
        "<title>icons</title>",
        "<button class=icon-btn><i class='fa fa-solid fa-trash'></i></button>",
        "<button class=icon-btn><svg width=16 height=16><use href='%23i-close'></use></svg></button>",
        "<div role=button tabindex=0 class=tb data-tooltip=Archive><svg width=16 height=16></svg></div>",
        "<a href='/x' class=nav><img src='/icons/home.png' width=16 height=16></a>",
        "<span class=close onclick=1 title='Close panel' style='display:inline-block;width:16px;height:16px'></span>",
        "<button class=x aria-describedby=tip><svg width=16 height=16></svg></button><div role=tooltip id=tip>Share</div>",
        // Nothing to call the icon by: what kind of icon it is, still.
        "<button><svg width=16 height=16><path d='M0 0'/></svg></button>",
        "<button><svg data-testid=DeleteIcon width=16 height=16></svg></button>",
        "<button><span style=\"display:inline-block;width:16px;height:16px;background:url('/i/bell.svg?v=2')\"></span></button>",
        "<button><img src='data:image/gif;base64,R0lGODlhAQABAAAAACw=' width=16 height=16></button>",
    );
    s.ok(&["goto", &format!("data:text/html,{page}")]);
    let tree = s.text(&["ax"]);
    for hint in [
        "button .icon-btn icon=\"fa-trash\" [ref=",
        "button .icon-btn icon=\"i-close\" [ref=",
        "button .tb data-tooltip=\"Archive\" [ref=",
        "link .nav img=\"home.png\" [ref=",
        "clickable .close title=\"Close panel\" [ref=",
        "button .x description=\"Share\" [ref=",
        "button icon=\"svg\" [ref=",
        "button icon=\"DeleteIcon\" [ref=",
        "button img=\"bell.svg\" [ref=",
        "button img=\"inline\" [ref=",
    ] {
        assert!(tree.contains(hint), "{hint}:\n{tree}");
    }
}

/// On these pages the tree built from the DOM snapshot (what a large page
/// gets) prints exactly what Chrome's accessibility tree does, refs
/// included: both are read from one page load (`NAVIGERA_AX_SOURCE=both`).
#[test]
fn the_dom_snapshot_tree_prints_what_chromes_tree_does() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let pages = [
        "/big-table",
        "/div-cells",
        "/long-text",
        "/cookie-banner",
        "/stale-ref",
        "/drag-native",
        "/drag-position",
        "/hidden-layers",
        "/iframe-form",
        "/srcdoc",
        "/multi-select",
        "/nbsp-select",
        "/shadow-css",
        "/overlapped",
        "/right-click",
        "/card-radios",
        "/combobox",
        "/typed-only",
        "/lazy-box",
        "/honesty",
        "/fillwait",
        "/effects",
    ];
    let mut input = String::new();
    for (i, page) in pages.iter().enumerate() {
        input.push_str(&format!(
            "{}\n",
            serde_json::json!({ "id": 1000 + i, "op": "goto", "url": format!("{}{page}", site.url) })
        ));
        input.push_str(&format!("{}\n", serde_json::json!({ "id": i, "op": "ax", "format": "text", "limit": 0 })));
    }
    // A snapshot scoped to one row of the large table.
    input.push_str(&format!(
        "{}\n",
        serde_json::json!({ "id": 2000, "op": "goto", "url": format!("{}/big-table", site.url) })
    ));
    input.push_str(&format!(
        "{}\n",
        serde_json::json!({ "id": 500, "op": "ax", "format": "text", "limit": 0, "selector": "#rows tr:nth-child(3)" })
    ));
    input.push_str("{\"id\":99999,\"op\":\"quit\"}\n");
    let mut child = Command::new(tool_exe())
        .args(["--own-browser", "serve"])
        .env("NAVIGERA_AX_SOURCE", "both")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("navigera serve");
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let mut checked = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let Some(i) = r["id"].as_u64().filter(|i| *i < 1000) else { continue };
        if i == 500 {
            let text = r["result"].as_str().unwrap_or_else(|| panic!("scoped: {r}"));
            let (chrome, dom) = text.split_once("\n=== DOM snapshot tree ===\n").unwrap();
            assert_eq!(dom, chrome, "a scoped snapshot differs");
            assert!(dom.contains("cell \"Item 2\"") && !dom.contains("Item 3"), "{dom}");
            checked += 1;
            continue;
        }
        let page = pages[i as usize];
        let text = r["result"].as_str().unwrap_or_else(|| panic!("{page}: {r}"));
        let (chrome, dom) =
            text.split_once("\n=== DOM snapshot tree ===\n").unwrap_or_else(|| panic!("{page}: {text}"));
        assert_eq!(dom, chrome, "{page}: the DOM snapshot tree differs from Chrome's");
        checked += 1;
    }
    assert_eq!(checked, pages.len() + 1);
}

/// `screenshot --quality` writes a JPEG; a value out of range is an error.
#[test]
fn a_screenshot_with_a_quality_is_a_jpeg() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("shot-jpeg", &[]);
    s.ok(&["goto", &format!("{}/stale-ref", site.url)]);
    let dir = std::env::temp_dir().join(format!("nv-jpeg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (jpg, png) = (dir.join("a.jpg"), dir.join("a.png"));
    let r = s.ok(&["screenshot", "--quality", "60", "--path", &jpg.display().to_string()]);
    assert!(r["bytes"].as_u64().unwrap_or(0) > 0, "{r}");
    s.ok(&["screenshot", "--path", &png.display().to_string()]);
    // `--timeout-ms` bounds the capture (a page that never answers must not hold it for 35 s).
    s.ok(&["screenshot", "--quality", "60", "--timeout-ms", "5000", "--path", &jpg.display().to_string()]);
    let (j, p) = (std::fs::read(&jpg).unwrap(), std::fs::read(&png).unwrap());
    assert_eq!(&j[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
    assert_eq!(&p[..4], b"\x89PNG", "a PNG without --quality");
    for bad in ["0", "101", "x"] {
        let out = Command::new(tool_exe())
            .args(["--session", &s.name, "screenshot", "--quality", bad])
            .stdin(Stdio::null())
            .output()
            .expect("run navigera");
        let e = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && e.contains("--quality takes 1 to 100"), "{bad}: {e}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A capture asked for right after a click that confirm()s, posts and reloads
/// the page used to go unanswered for the session's whole 35 s timeout.
#[test]
fn a_screenshot_right_after_a_confirmed_reload_is_not_lost() {
    let (true, Some(shop)) = (chrome(), bench_site()) else { return };
    let s = Session::start("confirm-shot", &[]);
    s.ok(&["goto", &format!("{}/shop/p/101", shop.url)]);
    s.ok(&["click", "--selector", "#cookie-accept"]);
    s.ok(&["click", "--selector", "#detail-add"]);
    s.ok(&["goto", &format!("{}/cart", shop.url)]);
    s.ok(&["dialog", "--accept"]);
    s.ok(&["click", "--selector", "[data-remove=\"101\"]"]);
    let dir = std::env::temp_dir().join(format!("nv-confirm-shot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let started = std::time::Instant::now();
    s.ok(&["screenshot", "--no-wait", "--timeout-ms", "10000", "--path", &dir.join("a.png").display().to_string()]);
    assert!(started.elapsed().as_secs_f64() < 3.0, "the capture took {:?}", started.elapsed());
    std::fs::remove_dir_all(&dir).ok();
}

fn bench_site() -> Option<Site> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/bench/site/server.py"))
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site { child, url })
}

/// Property changes with input/change/focus events wake a paused predicate;
/// a silent property assignment still has the timer fallback.
#[test]
fn property_events_wake_the_wait_after_the_pages_handlers() {
    if !chrome() {
        return;
    }
    let s = Session::start("event-wake", &[]);
    s.ok(&["goto", "data:text/html,<input id=c type=checkbox><input id=v><div id=m></div>"]);
    let js = format!(
        r#"(async () => {{
        const deep = {}, c = document.getElementById('c'), v = document.getElementById('v');
        let added = 0, removed = 0;
        const add = document.addEventListener.bind(document), remove = document.removeEventListener.bind(document);
        document.addEventListener = (...args) => {{ added++; return add(...args); }};
        document.removeEventListener = (...args) => {{ removed++; return remove(...args); }};
        const samples = [];
        for (const kind of ['change', 'input', 'focus', 'silent']) {{
            c.checked = false; v.blur(); v.value = ''; let changed = 0;
            const predicate = () => kind === 'focus' ? document.activeElement === v : kind === 'input' ? v.value === 'committed' : c.checked;
            // The page's handler must run before the wait is rechecked.
            const handler = () => {{ v.value = 'committed'; }};
            v.addEventListener('input', handler);
            setTimeout(() => {{
                if (kind === 'focus') v.focus();
                else if (kind === 'input') {{ v.value = 'draft'; v.dispatchEvent(new Event('input', {{ bubbles: true }})); }}
                else {{ c.checked = true; if (kind === 'change') c.dispatchEvent(new Event('change', {{ bubbles: true }})); }}
                changed = performance.now();
            }}, 25);
            while (!predicate()) await deep.wake(100);
            samples.push([kind, performance.now() - changed, predicate(), kind !== 'input' || v.value === 'committed']);
            v.removeEventListener('input', handler);
        }}
        return {{ samples, added, removed, value: v.value }};
    }})()"#,
        navigera::cdp::DEEP
    );
    let r = s.ok(&["eval", &js]);
    assert!(r["added"].as_u64().unwrap() >= 16, "listeners not installed: {r}");
    assert_eq!(r["added"], r["removed"], "listeners leaked: {r}");
    assert_eq!(r["samples"].as_array().unwrap().len(), 4, "{r}");
    // These are correctness checks, not machine-dependent performance gates.
    for row in r["samples"].as_array().unwrap() {
        assert!(row[1].as_f64().unwrap().is_finite(), "{r}");
        assert_eq!(row[2], true, "predicate not met: {r}");
        assert_eq!(row[3], true, "page handler not seen: {r}");
    }
}

/// Input events from fill and non-composed events in nested roots also wake
/// the pause. Every watched root loses its capture listeners afterwards.
#[test]
fn event_wakes_cover_fill_shadow_roots_and_frame_documents() {
    if !chrome() {
        return;
    }
    let s = Session::start("event-wake-roots", &[]);
    s.ok(&["goto", "data:text/html,<input id=v><div id=host></div><iframe></iframe>"]);
    let js = format!(
        r#"(() => {{
        const deep = {}, doc = document.querySelector('iframe').contentDocument;
        doc.body.innerHTML = '<input>';
        const shadow = document.getElementById('host').attachShadow({{mode:'open'}});
        shadow.innerHTML = '<input>';
        const roots = [document, shadow, doc], counts = roots.map(() => [0, 0]);
        roots.forEach((root, i) => {{
            const add = root.addEventListener.bind(root), remove = root.removeEventListener.bind(root);
            root.addEventListener = (...args) => {{ if (args[2] === true) counts[i][0]++; return add(...args); }};
            root.removeEventListener = (...args) => {{ if (args[2] === true) counts[i][1]++; return remove(...args); }};
        }});
        window.fillDone = (async () => {{
            const v = document.getElementById('v');
            while (v.value !== 'ready') await deep.wake(100);
            for (const root of [shadow, doc]) {{
                const input = root.querySelector('input');
                setTimeout(() => {{ input.value = 'ready'; input.dispatchEvent(new Event('input', {{bubbles:true, composed:false}})); }}, 25);
                while (input.value !== 'ready') await deep.wake(100);
            }}
            return counts;
        }})();
        return true;
    }})()"#,
        navigera::cdp::DEEP
    );
    s.ok(&["eval", &js]);
    s.ok(&["fill", "--selector", "#v", "ready"]);
    let counts = s.ok(&["eval", "window.fillDone"]);
    for row in counts.as_array().unwrap() {
        assert!(row[0].as_u64().unwrap() >= 12, "root not watched: {counts}");
        assert_eq!(row[0], row[1], "listeners leaked: {counts}");
    }
}

/// What a click puts in the page to watch it (a form's `submit`, for a
/// navigation it plans; the clipboard's writes, for an effect) is taken
/// out only while it is there: a method the page put in during the click
/// stays.
#[test]
fn a_click_leaves_the_methods_the_page_put_in_meanwhile() {
    let (true, Some(site)) = (chrome(), site()) else { return };
    let s = Session::start("own-methods", &[]);
    s.ok(&["goto", &format!("{}/nav-intent", site.url)]);
    s.ok(&[
        "eval",
        "document.querySelector('#f1').addEventListener('submit', () => { window.mine = function submit() {}; \
         HTMLFormElement.prototype.submit = window.mine; }); true",
    ]);
    s.ok(&["click", "--text", "Handled submit"]);
    assert_eq!(s.ok(&["eval", "HTMLFormElement.prototype.submit === window.mine"]), true);
    s.ok(&["goto", &format!("{}/effects", site.url)]);
    s.ok(&[
        "eval",
        "document.getElementById('copy').addEventListener('click', () => { window.mine = function writeText() \
         { return Promise.resolve(); }; Clipboard.prototype.writeText = window.mine; }); true",
    ]);
    s.ok(&["click", "--selector", "#copy"]);
    assert_eq!(s.ok(&["eval", "Clipboard.prototype.writeText === window.mine"]), true);
}

/// navigera's own page functions are called with the target and the values
/// as arguments, on the document's global object, which is kept between
/// calls (their text is then the same every time, and compiled once). A
/// document that replaced the one the object came from, by a `goto`, by
/// the page's own reload or by going back, gets the next call all the
/// same, and a value arrives as it was written: quotes, a backslash, a
/// line break, a template placeholder, text outside the BMP.
#[test]
fn actions_follow_the_document_and_values_arrive_as_written() {
    if !chrome() {
        return;
    }
    let s = Session::start("const-calls", &[]);
    let value = "a \"b\" 'c' \\ `d` ${e}\nline two \u{1F980}";
    let read = "(() => document.querySelector('[aria-label=\"it\\'s\"]').value)()";
    for n in 0..3 {
        let page = format!(
            "<title>page {n}</title><textarea aria-label=\"it's\"></textarea>\
             <button onclick=\"document.title='clicked {n}'\">Go {n}</button>"
        );
        s.ok(&["goto", &format!("data:text/html,{page}")]);
        s.ok(&["fill", "--selector", "[aria-label=\"it's\"]", "--value", value]);
        assert_eq!(s.ok(&["eval", read]), value);
        s.ok(&["click", "--text", &format!("Go {n}")]);
        assert_eq!(s.ok(&["title"]), format!("clicked {n}"));
    }
    // The page replaces its own document: nothing told navigera first.
    s.ok(&["eval", "(() => { location.reload(); return 1 })()"]);
    s.ok(&["click", "--text", "Go 2"]);
    assert_eq!(s.ok(&["title"]), "clicked 2");
    s.ok(&["back"]);
    s.ok(&["click", "--text", "Go 1"]);
    assert_eq!(s.ok(&["title"]), "clicked 1");
    s.ok(&["ax"]);
    s.ok(&["fill", "--selector", "textarea", "--value", "x"]);
    s.ok(&["click", "--text", "Go 1"]);
    assert_eq!(s.ok(&["eval", read]), "x");
}
