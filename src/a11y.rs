//! `a11y`: an accessibility audit of the page with axe-core (Deque, MPL-2.0,
//! vendored unmodified in `vendor/axe-core`), the engine behind most
//! accessibility checkers. It runs in an isolated world of the page, so the
//! page never sees a `window.axe`, and each element a rule flags comes back
//! as a ref an agent can act on (`click 12`, `ax 12`).

use std::sync::OnceLock;

use anyhow::{bail, Result};
use serde_json::{json, Value};

/// axe-core's version, as vendored.
pub const AXE_VERSION: &str = "4.14.0";

/// WCAG 2.0, 2.1 and 2.2 at levels A and AA: what accessibility law and
/// most audits ask for.
pub const DEFAULT_TAGS: &[&str] = &["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];

/// The elements shown per rule; the rest are counted.
pub const NODES_PER_RULE: usize = 5;

static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/axe.min.js.deflate"));

/// axe-core's script, inflated once.
pub fn axe_source() -> &'static str {
    static SOURCE: OnceLock<String> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let raw = miniz_oxide::inflate::decompress_to_vec(PACKED).expect("the embedded axe-core inflates");
        String::from_utf8(raw).expect("axe-core is UTF-8")
    })
}

/// The script that loads axe into the isolated world it runs in, once.
pub fn load_js() -> String {
    format!(
        "(() => {{ if (typeof axe === 'object' && axe.run) return true;\n{}\nreturn typeof axe === 'object'; }})()",
        axe_source()
    )
}

/// The run: violations, the worst first, each with its first elements;
/// those elements are kept in `globalThis.__nv_axe_nodes` for the refs.
pub fn run_js(selector: Option<&str>, tags: &[String], ignore: &[String]) -> String {
    format!(
        r#"(async () => {{
  const sel = {sel};
  if (sel && !document.querySelector(sel)) throw new Error('a11y --selector ' + JSON.stringify(sel) + ': no element matches');
  const r = await axe.run(sel ? {{ include: [sel] }} : document, {{
    runOnly: {{ type: 'tag', values: {tags} }}, resultTypes: ['violations'], iframes: false, elementRef: true }});
  const ignore = {ignore};
  const order = {{ critical: 0, serious: 1, moderate: 2, minor: 3 }};
  const kept = [];
  const target = (t) => t.map((p) => Array.isArray(p) ? p.join(' >>> ') : p).join(' ');
  const rules = r.violations.filter((v) => !ignore.includes(v.id))
    .sort((a, b) => (order[a.impact] ?? 4) - (order[b.impact] ?? 4))
    .map((v) => ({{ rule: v.id, impact: v.impact, help: v.help, count: v.nodes.length,
      nodes: v.nodes.slice(0, {per}).map((n) => {{
        kept.push(n.element || null);
        return {{ at: kept.length - 1, selector: target(n.target),
                 html: n.html.length > 120 ? n.html.slice(0, 119) + '…' : n.html }};
      }}) }}));
  globalThis.__nv_axe_nodes = kept;
  return rules;
}})()"#,
        sel = json!(selector),
        tags = json!(tags),
        ignore = json!(ignore),
        per = NODES_PER_RULE,
    )
}

/// `--tags`/`--ignore` as lists (`a,b c` and repeats both work).
pub fn list(raw: &[String]) -> Vec<String> {
    raw.iter().flat_map(|r| r.split([',', ' '])).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// The answer: `{violations, rules: [...], ...}`, each node with its ref
/// (`refs[at]`) in place of the index it was kept at.
pub fn finish(rules: Value, refs: &[Option<u64>], tags: &[String]) -> Result<Value> {
    let Value::Array(mut rules) = rules else { bail!("axe answered {rules}") };
    let mut total = 0;
    let mut by_impact = serde_json::Map::new();
    for rule in &mut rules {
        let count = rule["count"].as_u64().unwrap_or(0);
        total += count;
        let impact = rule["impact"].as_str().unwrap_or("unknown").to_string();
        let seen = by_impact.get(&impact).and_then(Value::as_u64).unwrap_or(0);
        by_impact.insert(impact, json!(seen + count));
        if let Some(nodes) = rule["nodes"].as_array_mut() {
            for n in nodes.iter_mut() {
                let at = n["at"].as_u64().unwrap_or(u64::MAX) as usize;
                if let Some(Some(r)) = refs.get(at) {
                    n["ref"] = json!(r);
                }
                n.as_object_mut().map(|o| o.remove("at"));
            }
            let shown = nodes.len() as u64;
            if count > shown {
                rule["more"] = json!(count - shown);
            }
        }
    }
    Ok(json!({
        "violations": total,
        "by_impact": by_impact,
        "rules": rules,
        "tags": tags,
        "axe": AXE_VERSION,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_axe_is_the_vendored_one() {
        let src = axe_source();
        assert!(src.len() > 600_000 && src.contains("axe v4.14.0"), "{} bytes", src.len());
        assert!(PACKED.len() < 250_000, "stored deflated: {} bytes", PACKED.len());
    }

    #[test]
    fn results_carry_refs_counts_and_what_is_left_out() {
        let rules = json!([
            {"rule": "image-alt", "impact": "critical", "help": "Images must have alternative text", "count": 7,
             "nodes": [{"at": 0, "selector": "img", "html": "<img>"}, {"at": 1, "selector": "img.b", "html": "<img class=b>"}]},
            {"rule": "region", "impact": "moderate", "help": "All page content should be contained by landmarks", "count": 1,
             "nodes": [{"at": 2, "selector": "p", "html": "<p>"}]}
        ]);
        let out = finish(rules, &[Some(12), None, Some(30)], &list(&["wcag2a, wcag2aa".into()])).unwrap();
        assert_eq!(out["violations"], 8);
        assert_eq!(out["by_impact"], json!({"critical": 7, "moderate": 1}));
        assert_eq!(out["rules"][0]["nodes"][0], json!({"selector": "img", "html": "<img>", "ref": 12}));
        assert!(out["rules"][0]["nodes"][1].get("ref").is_none(), "an element with no node id has no ref");
        assert_eq!(out["rules"][0]["more"], 5);
        assert!(out["rules"][1].get("more").is_none());
        assert_eq!(out["tags"], json!(["wcag2a", "wcag2aa"]));
    }
}
