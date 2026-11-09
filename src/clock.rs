//! `clock`: the page's time under the test's control. `Date`,
//! `performance.now`, `setTimeout`/`setInterval` (and their clears) and
//! `Intl.DateTimeFormat` without a date read a clock navigera keeps in
//! every document of every tab: started at a given time, flowing with real
//! time or paused, jumped, or run forward with the timers due on the way
//! firing in order. Animation frames and idle callbacks keep real time.
//!
//! navigera's own scripts in the page (its waits, its settle proof) keep
//! the real clock, or a paused clock would stop them: a call whose stack
//! holds no frame with a source location (only `<anonymous>` ones, which is
//! what `Runtime.evaluate` code shows) is navigera's. `eval` marks its code
//! with a source URL of its own ([`USER_EVAL_URL`]), so a script a person
//! runs sees the page's clock.

use anyhow::{bail, Result};

/// The source URL `eval` gives the code it runs, so the clock counts it as
/// the page's.
pub const USER_EVAL_URL: &str = "navigera-eval";

/// The source URL of the clock's own code: its frames are neither the
/// page's nor navigera's.
pub const CLOCK_URL: &str = "navigera-clock";

/// `(CLOCK_JS)(cfg)` as a script, marked with [`CLOCK_URL`].
pub fn install_script(cfg: &serde_json::Value) -> String {
    format!("({CLOCK_JS})({cfg})\n//# sourceURL={CLOCK_URL}\n")
}

/// The clock, installed by `(CLOCK_JS)(cfg)`: `cfg.time` (a date string or
/// epoch ms) or `cfg.now` (epoch ms at real time `cfg.at`, when the
/// document that has it was not yet open) starts it, `cfg.paused` holds it.
/// Again in a document that has one: the clock is set to `cfg` (timers
/// kept). Returns its state.
pub const CLOCK_JS: &str = r#"(cfg) => {
    const W = window;
    if (W.__nvClock) return W.__nvClock.configure(cfg);
    const RD = W.Date, rnow = RD.now.bind(RD), P = W.performance, rperf = P.now.bind(P);
    const RE = W.Error, IDF = W.Intl && W.Intl.DateTimeFormat;
    // navigera's settle hooks may hold the page's timer functions: the
    // originals are theirs, and their next arming wraps ours.
    const nv = W.__nvMut;
    if (nv && nv.unT) nv.unT();
    const T = nv && nv.T ? { st: nv.T.st, ct: nv.T.ct, si: nv.T.si, ci: nv.T.ci }
        : { st: W.setTimeout, ct: W.clearTimeout, si: W.setInterval, ci: W.clearInterval };
    const SRC = /:\d+:\d+\)?$/, CLOCK_URL = 'navigera-clock';
    // The page's call (a frame with a source location, or `eval`'s marked
    // code), not one of navigera's own scripts.
    const pageCall = () => {
        const lim = RE.stackTraceLimit;
        RE.stackTraceLimit = 16;
        const s = new RE().stack || '';
        RE.stackTraceLimit = lim;
        const lines = s.split('\n');
        for (let i = 1; i < lines.length; i++) {
            const l = lines[i].trim();
            if (SRC.test(l) && !l.includes('<anonymous>') && !l.includes(CLOCK_URL)) return true;
        }
        return false;
    };
    // `mono`: the timers' time (ms since install, never backward); the
    // date is mono + skew.
    const k = { mono: 0, real: rnow(), paused: false, skew: 0, timers: new Map(), seq: 1 << 30, wake: 0, running: false, perf0: rperf() };
    const mono = () => (k.paused || k.running) ? k.mono : k.mono + (rnow() - k.real);
    const vnow = () => mono() + k.skew;
    const hold = () => { k.mono = mono(); k.real = rnow(); };
    const parse = (t) => {
        const n = typeof t === 'number' ? t : (/^\s*-?\d+(\.\d+)?\s*$/.test(String(t)) ? Number(t) : new RD(String(t)).getTime());
        if (!Number.isFinite(n)) throw new RE('clock: not a time: ' + JSON.stringify(t) + ' (a date like 2024-02-02T08:00:00, or epoch ms)');
        return n;
    };
    const fire = (t) => {
        try {
            if (typeof t.f === 'function') t.f.apply(W, t.a);
            else (0, W.eval)(String(t.f));
        } catch (e) {
            // An uncaught error, as a real timer's would be.
            T.st.call(W, () => { throw e; }, 0);
        }
    };
    const next = (upTo) => {
        let best = null;
        for (const t of k.timers.values()) {
            if (t.due > upTo) continue;
            if (!best || t.due < best.due || (t.due === best.due && t.id < best.id)) best = t;
        }
        return best;
    };
    const schedule = () => {
        if (k.wake) { T.ct.call(W, k.wake); k.wake = 0; }
        if (k.paused || k.running || !k.timers.size) return;
        let due = Infinity;
        for (const t of k.timers.values()) due = Math.min(due, t.due);
        k.wake = T.st.call(W, wake, Math.max(0, due - mono()));
    };
    const runOne = (t, now) => {
        if (t.every) t.due = Math.max(t.due + t.every, now); else k.timers.delete(t.id);
        fire(t);
    };
    function wake() {
        k.wake = 0;
        const now = mono();
        // Flowing: what is due now fires, each timer once per wake.
        const due = [...k.timers.values()].filter((t) => t.due <= now).sort((a, b) => a.due - b.due || a.id - b.id);
        for (const t of due) if (k.timers.get(t.id) === t) runOne(t, now);
        schedule();
    }
    // Run the timers due up to `to` (mono) in order, the clock at each
    // one's time; `once`: each timer at most once (a jump).
    const runTo = (to, once) => {
        hold();
        k.running = true;
        let fired = 0;
        try {
            if (once) {
                const due = [...k.timers.values()].filter((t) => t.due <= to).sort((a, b) => a.due - b.due || a.id - b.id);
                k.mono = Math.max(k.mono, to);
                for (const t of due) if (k.timers.get(t.id) === t) { runOne(t, to); fired++; }
            } else {
                for (let t = next(to); t; t = next(to)) {
                    k.mono = Math.max(k.mono, t.due);
                    runOne(t, t.due);
                    if (++fired > 100000) throw new RE('clock: more than 100000 timers fired: a timer that schedules itself without end?');
                }
                k.mono = Math.max(k.mono, to);
            }
        } finally {
            k.running = false;
            k.real = rnow();
            schedule();
        }
        return fired;
    };
    const add = (f, d, a, every) => {
        const id = ++k.seq;
        d = Math.max(0, Number(d) || 0);
        k.timers.set(id, { id, f, a, due: mono() + d, every: every ? Math.max(1, d) : 0 });
        schedule();
        return id;
    };
    const clear = (id) => {
        if (k.timers.delete(id)) { schedule(); return true; }
        return false;
    };
    const fakes = {
        setTimeout: { setTimeout(f, d, ...a) { return pageCall() ? add(f, d, a, false) : T.st.call(W, f, d, ...a); } }.setTimeout,
        setInterval: { setInterval(f, d, ...a) { return pageCall() ? add(f, d, a, true) : T.si.call(W, f, d, ...a); } }.setInterval,
        clearTimeout: { clearTimeout(id) { if (!clear(id)) T.ct.call(W, id); } }.clearTimeout,
        clearInterval: { clearInterval(id) { if (!clear(id)) T.ci.call(W, id); } }.clearInterval,
    };
    function Date(...a) {
        const fake = !a.length && pageCall();
        if (!new.target) return fake ? new RD(vnow()).toString() : RD();
        return Reflect.construct(RD, a.length ? a : fake ? [vnow()] : [], new.target === Date ? RD : new.target);
    }
    Date.prototype = RD.prototype;
    Date.now = { now() { return pageCall() ? Math.floor(vnow()) : rnow(); } }.now;
    Date.parse = RD.parse;
    Date.UTC = RD.UTC;
    Object.defineProperty(Date, 'name', { value: 'Date' });
    Object.defineProperty(Date, 'length', { value: 7 });
    const native = (f, name) => Object.defineProperty(f, 'toString', { value: () => 'function ' + name + '() { [native code] }', configurable: true });
    native(Date, 'Date'); native(Date.now, 'now');
    for (const n in fakes) { native(fakes[n], n); W[n] = fakes[n]; }
    RD.prototype.constructor = Date;
    W.Date = Date;
    const perfNow = { now() { return pageCall() ? k.perf0 + mono() : rperf(); } }.now;
    native(perfNow, 'now');
    try { Object.defineProperty(P, 'now', { value: perfNow, configurable: true, writable: true }); } catch (_) {}
    if (IDF) {
        for (const m of ['format', 'formatToParts']) {
            const d = Object.getOwnPropertyDescriptor(IDF.prototype, m);
            if (!d) continue;
            const orig = d.get ? d.get : d.value;
            if (d.get) {
                Object.defineProperty(IDF.prototype, m, { configurable: true, get() {
                    const f = orig.call(this);
                    return (date) => f(date === undefined && pageCall() ? vnow() : date);
                } });
            } else {
                Object.defineProperty(IDF.prototype, m, { configurable: true, writable: true, value: function (date) {
                    return orig.call(this, date === undefined && pageCall() ? vnow() : date);
                } });
            }
        }
    }
    if (nv) nv.T = { st: fakes.setTimeout, ct: fakes.clearTimeout, si: fakes.setInterval, ci: fakes.clearInterval };
    const state = () => ({ now: Math.floor(vnow()), time: new RD(vnow()).toISOString(), paused: k.paused, timers: k.timers.size });
    const api = {
        state,
        paused: () => k.paused,
        configure(c) {
            hold();
            let now;
            if (c.time !== undefined && c.time !== null) now = parse(c.time);
            else now = c.paused ? c.now : c.now + Math.max(0, rnow() - c.at);
            k.skew = now - k.mono;
            k.paused = !!c.paused;
            schedule();
            return state();
        },
        // The date jumps; the timers keep their delays.
        set(t) { hold(); k.skew = parse(t) - k.mono; schedule(); return state(); },
        runFor(ms) { const fired = runTo(mono() + ms, false); return Object.assign(state(), { fired }); },
        fastForward(ms) { const fired = runTo(mono() + ms, true); return Object.assign(state(), { fired }); },
        pauseAt(t) {
            // Stopped first: a flowing clock paused after the run picked up
            // the real milliseconds that went by meanwhile (01:00:00.001
            // for a pause at 01:00, on a slow runner).
            hold(); k.paused = true;
            const at = parse(t) - k.skew;
            let fired = 0;
            if (at > k.mono) fired = runTo(at, false); else k.skew = parse(t) - k.mono;
            schedule();
            return Object.assign(state(), { fired });
        },
        resume() { hold(); k.paused = false; schedule(); return state(); },
    };
    Object.defineProperty(W, '__nvClock', { value: api, configurable: true, enumerable: false });
    return api.configure(cfg);
}"#;

/// A `clock` length: `500ms`, `10s`, `30m`, `1h`, `2d`, `mm:ss`, `hh:mm:ss`,
/// or a number of milliseconds.
pub fn parse_duration(s: &str) -> Result<f64> {
    let t = s.trim();
    let bad = || anyhow::anyhow!("clock: not a length of time: {s:?} (500ms, 10s, 30m, 1h, 2d, mm:ss or hh:mm:ss)");
    if t.contains(':') {
        let parts: Vec<&str> = t.split(':').collect();
        if parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
            return Err(bad());
        }
        let mut secs = 0f64;
        for p in &parts {
            secs = secs * 60.0 + p.parse::<f64>().map_err(|_| bad())?;
        }
        return Ok(secs * 1000.0);
    }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().map_err(|_| bad())?;
    let ms = match unit.trim() {
        "" | "ms" => n,
        "s" => n * 1e3,
        "m" | "min" => n * 60e3,
        "h" => n * 3600e3,
        "d" => n * 86_400e3,
        _ => return Err(bad()),
    };
    if !ms.is_finite() || ms < 0.0 {
        return Err(bad());
    }
    Ok(ms)
}

/// What `clock <action>` does, checked before it reaches a page.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// `clock` alone: the state.
    Show,
    Install {
        time: Option<String>,
        paused: bool,
    },
    Set(String),
    RunFor(f64),
    FastForward(f64),
    PauseAt(String),
    Resume,
    Uninstall,
}

impl Op {
    pub fn parse(action: Option<&str>, value: Option<&str>, time: Option<&str>, paused: bool) -> Result<Op> {
        let need = |what: &str| -> Result<String> {
            value.map(str::to_string).ok_or_else(|| anyhow::anyhow!("clock {}: give {what}", action.unwrap_or("")))
        };
        let op = match action {
            None => Op::Show,
            Some("install") => Op::Install { time: time.or(value).map(str::to_string), paused },
            Some("set") | Some("set-time") => Op::Set(need("a time (2024-02-02T08:00:00, or epoch ms)")?),
            Some("run-for") | Some("run") | Some("tick") => Op::RunFor(parse_duration(&need("a length (30s, 5m)")?)?),
            Some("fast-forward") | Some("jump") => Op::FastForward(parse_duration(&need("a length (30s, 5m)")?)?),
            Some("pause-at") | Some("pause") => Op::PauseAt(need("a time (2024-02-02T08:00:00, or epoch ms)")?),
            Some("resume") => Op::Resume,
            Some("uninstall") | Some("off") => Op::Uninstall,
            Some(other) => bail!(
                "clock: {other:?} is not an action: install [--time <t>] [--paused], set <t>, run-for <length>, fast-forward <length>, pause-at <t>, resume, uninstall"
            ),
        };
        if !matches!(op, Op::Install { .. }) && (time.is_some() || paused) {
            bail!("clock: --time and --paused go with install");
        }
        Ok(op)
    }

    /// The call into an installed clock.
    pub fn call(&self) -> Option<String> {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        Some(match self {
            Op::Show => "state()".into(),
            Op::Set(t) => format!("set({})", q(t)),
            Op::RunFor(ms) => format!("runFor({ms})"),
            Op::FastForward(ms) => format!("fastForward({ms})"),
            Op::PauseAt(t) => format!("pauseAt({})", q(t)),
            Op::Resume => "resume()".into(),
            Op::Install { .. } | Op::Uninstall => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_as_playwright_writes_them() {
        assert_eq!(parse_duration("500").unwrap(), 500.0);
        assert_eq!(parse_duration("500ms").unwrap(), 500.0);
        assert_eq!(parse_duration("1.5s").unwrap(), 1500.0);
        assert_eq!(parse_duration("30m").unwrap(), 1_800_000.0);
        assert_eq!(parse_duration("2h").unwrap(), 7_200_000.0);
        assert_eq!(parse_duration("1d").unwrap(), 86_400_000.0);
        assert_eq!(parse_duration("30:00").unwrap(), 1_800_000.0);
        assert_eq!(parse_duration("01:00:05").unwrap(), 3_605_000.0);
        for bad in ["", "x", "5 parsecs", "1:2:3:4", "-1s", "1::2"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn actions_parse_and_call_the_clock() {
        let p = |a: Option<&str>, v: Option<&str>| Op::parse(a, v, None, false).unwrap();
        assert_eq!(p(None, None), Op::Show);
        assert_eq!(p(Some("run-for"), Some("30m")).call().unwrap(), "runFor(1800000)");
        assert_eq!(p(Some("pause-at"), Some("2024-02-02 10:00")).call().unwrap(), "pauseAt(\"2024-02-02 10:00\")");
        assert_eq!(
            Op::parse(Some("install"), None, Some("2024-02-02"), true).unwrap(),
            Op::Install { time: Some("2024-02-02".into()), paused: true }
        );
        assert!(Op::parse(Some("run-for"), None, None, false).unwrap_err().to_string().contains("give a length"));
        assert!(Op::parse(Some("resume"), None, None, true).is_err());
        assert!(Op::parse(Some("warp"), None, None, false).unwrap_err().to_string().contains("not an action"));
    }
}
