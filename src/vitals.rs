//! `vitals`: the current document's web vitals as the browser measured
//! them. Nothing is injected beforehand: every number comes from the
//! performance entries Chrome buffers for each document, read in one
//! evaluation.
//!
//! INP comes from the browser's buffer of event timings, which keeps every
//! interaction of 104 ms or more (the Event Timing spec's buffering
//! threshold). An INP of 104 ms or more is therefore exact; when the
//! page's interactions were all quicker, it is reported as under 104 ms
//! (`inp: null`, `inp_under_ms: 104`) rather than guessed.

use serde_json::{json, Value};

/// Google's thresholds (good up to the first, poor from the second).
const THRESHOLDS: &[(&str, f64, f64)] = &[
    ("ttfb", 800.0, 1800.0),
    ("fcp", 1800.0, 3000.0),
    ("lcp", 2500.0, 4000.0),
    ("cls", 0.1, 0.25),
    ("inp", 200.0, 500.0),
];

/// The browser buffers event timings of this many ms or more.
pub const INP_BUFFERED_MS: f64 = 104.0;

/// Reads the vitals in one evaluation; [`finish`] rates them.
pub const JS: &str = r#"(() => {
  const take = (type, extra) => {
    try {
      const o = new PerformanceObserver(() => {});
      o.observe(Object.assign({ type, buffered: true }, extra || {}));
      const got = o.takeRecords();
      o.disconnect();
      return got;
    } catch (_) { return []; }
  };
  const ms = (n) => (n == null ? null : Math.round(n * 10) / 10);
  // A short CSS selector that finds the element, for --selector.
  const sel = (el) => {
    if (el && el.nodeType === 3) el = el.parentElement;
    if (!el || el.nodeType !== 1) return null;
    const one = (e) => {
      if (e.id && /^[A-Za-z][\w-]*$/.test(e.id)) return '#' + e.id;
      let s = e.localName;
      const cls = [...e.classList].filter((c) => /^[A-Za-z_][\w-]*$/.test(c)).slice(0, 2);
      if (cls.length) s += '.' + cls.join('.');
      const p = e.parentElement;
      if (p) {
        const same = [...p.children].filter((c) => c.localName === e.localName);
        if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(e) + 1) + ')';
      }
      return s;
    };
    let path = one(el);
    for (let e = el.parentElement, i = 0; e && i < 4; e = e.parentElement, i++) {
      try { if (document.querySelectorAll(path).length === 1) break; } catch (_) { break; }
      if (e === document.body || e === document.documentElement) break;
      path = one(e) + ' > ' + path;
    }
    return path;
  };
  const nav = performance.getEntriesByType('navigation')[0];
  const out = { url: location.href };
  out.ttfb = nav ? ms(nav.responseStart) : null;
  const fcp = performance.getEntriesByName('first-contentful-paint')[0];
  out.fcp = fcp ? ms(fcp.startTime) : null;
  const lcps = take('largest-contentful-paint');
  const lcp = lcps[lcps.length - 1];
  out.lcp = lcp ? ms(lcp.startTime) : null;
  if (lcp) out.lcp_element = sel(lcp.element) || lcp.url || null;
  out.load = nav && nav.loadEventEnd > 0 ? ms(nav.loadEventEnd) : null;
  // CLS: the largest session window (gaps under 1 s, at most 5 s long) of
  // shifts that no input caused.
  let best = 0, bestWindow = [], sum = 0, first = 0, last = 0, win = [];
  for (const e of take('layout-shift').filter((e) => !e.hadRecentInput)) {
    if (sum && (e.startTime - last > 1000 || e.startTime - first > 5000)) { sum = 0; win = []; }
    if (!sum) first = e.startTime;
    sum += e.value; last = e.startTime; win.push(e);
    if (sum > best) { best = sum; bestWindow = win.slice(); }
  }
  out.cls = Math.round(best * 10000) / 10000;
  if (best > 0) {
    const moved = [];
    for (const e of bestWindow.slice().sort((a, b) => b.value - a.value)) {
      for (const s of e.sources || []) {
        const p = sel(s.node);
        if (p && !moved.includes(p)) moved.push(p);
      }
    }
    out.cls_elements = moved.slice(0, 3);
  }
  // INP: the slowest interaction, one in 50 left out (the 98th percentile).
  // An interaction's time is its longest event's; it is named by the event
  // whose handlers ran longest, where the time went.
  const slowest = new Map();
  const busy = (e) => e.processingEnd - e.processingStart;
  for (const e of take('event', { durationThreshold: 16 })) {
    if (!e.interactionId) continue;
    const had = slowest.get(e.interactionId);
    if (!had) slowest.set(e.interactionId, { duration: e.duration, by: e });
    else {
      had.duration = Math.max(had.duration, e.duration);
      if (busy(e) > busy(had.by)) had.by = e;
    }
  }
  const kept = [...slowest.values()].sort((a, b) => b.duration - a.duration);
  const count = typeof performance.interactionCount === 'number' ? Math.max(performance.interactionCount, kept.length) : kept.length;
  out.interactions = count;
  const at = Math.floor(count / 50);
  if (at < kept.length) {
    const e = kept[at];
    out.inp = e.duration;
    out.inp_event = e.by.name;
    out.inp_element = sel(e.by.target);
  } else {
    out.inp = null;
  }
  return out;
})()"#;

/// Adds what the page can't say: how each number rates, and that an INP
/// the browser didn't keep is under its buffering threshold.
pub fn finish(mut v: Value) -> Value {
    let interactions = v["interactions"].as_u64().unwrap_or(0);
    if v["inp"].is_null() && interactions > 0 {
        v["inp_under_ms"] = json!(INP_BUFFERED_MS);
    }
    let mut rating = serde_json::Map::new();
    for (name, good, poor) in THRESHOLDS {
        let r = match v[*name].as_f64() {
            Some(n) => rate(n, *good, *poor),
            // Every interaction was quicker than 104 ms: good.
            None if *name == "inp" && interactions > 0 => "good",
            None => continue,
        };
        rating.insert(name.to_string(), json!(r));
    }
    v["rating"] = Value::Object(rating);
    v
}

fn rate(n: f64, good: f64, poor: f64) -> &'static str {
    if n <= good {
        "good"
    } else if n <= poor {
        "needs-improvement"
    } else {
        "poor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_by_googles_thresholds() {
        let v = finish(
            json!({"ttfb": 120.0, "fcp": 1900.0, "lcp": 4200.0, "cls": 0.1, "inp": 230.0, "interactions": 3, "load": 900.0}),
        );
        assert_eq!(
            v["rating"],
            json!({"ttfb": "good", "fcp": "needs-improvement", "lcp": "poor", "cls": "good", "inp": "needs-improvement"})
        );
        assert!(v.get("inp_under_ms").is_none());
    }

    #[test]
    fn an_inp_the_browser_did_not_keep_is_under_its_threshold() {
        let v = finish(json!({"lcp": null, "cls": 0.0, "inp": null, "interactions": 2}));
        assert_eq!(v["inp_under_ms"], 104.0);
        assert_eq!(v["rating"], json!({"cls": "good", "inp": "good"}));
        // No interaction yet: nothing to say about INP.
        let v = finish(json!({"cls": 0.0, "inp": null, "interactions": 0}));
        assert!(v.get("inp_under_ms").is_none() && v["rating"].get("inp").is_none());
    }
}
