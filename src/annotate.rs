//! `screenshot --annotate`: each ref of the page outlined and labelled
//! with its number in the image, so a model that reads screenshots can
//! say `click 12` about what it sees. The marks live in a closed shadow
//! root (the page's styles can't reach them), are `aria-hidden` with no
//! pointer events, and are removed right after the capture.

use serde_json::json;

/// The refs an `ax` text tree shows (`[ref=12]`), in order, each once.
pub fn refs_in(tree: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for part in tree.split("[ref=").skip(1) {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u64>() {
            if part[digits.len()..].starts_with(']') && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// The script that draws the marks: `(ref, x, y, width, height)` in
/// viewport CSS pixels. It answers once a frame with the marks was painted.
pub fn draw(boxes: &[(u64, f64, f64, f64, f64)]) -> String {
    let marks = json!(boxes.iter().map(|&(id, x, y, w, h)| json!([id, x, y, w, h])).collect::<Vec<_>>());
    format!(
        r#"(() => {{
  document.getElementById('__nv_marks')?.remove();
  const host = document.createElement('div');
  host.id = '__nv_marks';
  host.setAttribute('aria-hidden', 'true');
  host.style.cssText = 'all:initial;position:absolute;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none';
  const root = host.attachShadow({{ mode: 'closed' }});
  const sx = scrollX, sy = scrollY;
  for (const [id, x, y, w, h] of {marks}) {{
    const b = document.createElement('div');
    b.style.cssText = `position:absolute;left:${{x + sx}}px;top:${{y + sy}}px;width:${{w}}px;height:${{h}}px;box-sizing:border-box;border:2px solid #e6007e`;
    const l = document.createElement('div');
    l.textContent = id;
    l.style.cssText = `position:absolute;left:${{x + sx}}px;top:${{(y >= 14 ? y - 14 : y) + sy}}px;font:bold 11px/13px monospace;color:#fff;background:#e6007e;padding:0 3px;border-radius:2px;white-space:nowrap`;
    root.append(b, l);
  }}
  document.documentElement.append(host);
  // The capture is of what was painted: wait for a frame that has the marks
  // (two animation frames; a tab that paints nothing gets 250 ms, not forever).
  return new Promise((done) => {{
    setTimeout(() => done(true), 250);
    requestAnimationFrame(() => requestAnimationFrame(() => done(true)));
  }});
}})()"#
    )
}

/// Takes the marks away.
pub const REMOVE: &str = "(() => { document.getElementById('__nv_marks')?.remove(); return true; })()";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_come_from_the_tree_in_order_once() {
        let tree = "page \"Shop\"\n- button \"Buy\" [ref=12]\n- link \"Cart\" [ref=7]\n- text: [ref=x] not one\n- button \"Buy\" [ref=12]";
        assert_eq!(refs_in(tree), vec![12, 7]);
        assert!(refs_in("no refs here").is_empty());
    }

    #[test]
    fn the_script_carries_every_box() {
        let js = draw(&[(12, 10.0, 20.0, 30.0, 40.0)]);
        assert!(js.contains("[[12,10.0,20.0,30.0,40.0]]") && js.contains("attachShadow"), "{js}");
    }
}
