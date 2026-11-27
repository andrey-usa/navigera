//! A document that is not the page the agent asked for: the server's error
//! page (a 4xx or 5xx status) or a bot challenge. Read with the URL and the
//! title an action's answer reports (no round trip of its own), and said
//! once per document, with what to do next.

use serde_json::{json, Value};

/// `[status, timeOrigin, challenge]` of the document now shown:
/// - its navigation entry's HTTP status: 0 where the engine doesn't say
///   (Safari has no `responseStatus`) or the page didn't come
///   over HTTP;
/// - its time origin, which tells documents apart: a new document has a
///   new one, a page restored from the back/forward cache keeps its own;
/// - `"cloudflare"` on Cloudflare's challenge page (its script defines
///   `_cf_chl_opt`; the bot-detection script Cloudflare adds to ordinary
///   pages doesn't), else null.
///
/// null if the page throws on any of it.
pub const DOC_JS: &str = "(() => { try { const n = performance.getEntriesByType('navigation')[0]; \
     return [(n && n.responseStatus) || 0, performance.timeOrigin, \
     typeof window._cf_chl_opt === 'object' && window._cf_chl_opt !== null ? 'cloudflare' : null]; \
     } catch (_) { return null; } })()";

/// What [`DOC_JS`] read.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub status: u16,
    pub origin: f64,
    pub challenge: Option<&'static str>,
}

impl Doc {
    pub fn parse(v: &Value) -> Option<Doc> {
        let a = v.as_array()?;
        let status = a.first()?.as_u64().filter(|s| *s < 1000).unwrap_or(0) as u16;
        let origin = a.get(1)?.as_f64()?;
        let challenge = match a.get(2).and_then(Value::as_str) {
            Some("cloudflare") => Some("cloudflare"),
            _ => None,
        };
        Some(Doc { status, origin, challenge })
    }

    /// Worth a second source: the status is unknown, or it is one a bot
    /// wall answers with (AWS WAF's 202 and 405, Cloudflare's 403, Vercel's
    /// 429) or an error and no challenge was seen on the page.
    pub fn wants_more(&self) -> bool {
        self.status == 0 || (self.challenge.is_none() && (self.status == 202 || self.status >= 400))
    }
}

/// A bot challenge the response itself declares: Cloudflare (403 with
/// `cf-mitigated: challenge`), Vercel (429 with `x-vercel-mitigated:
/// challenge`), AWS WAF (202 or 405 with `x-amzn-waf-action: captcha` or
/// `challenge`). `headers` is a JSON object of header names (any case) to
/// values.
pub fn challenge_from_headers(status: u64, headers: &Value) -> Option<&'static str> {
    let get = |name: &str| {
        headers.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|(_, v)| v.as_str())
    };
    let is =
        |name: &str, words: &[&str]| get(name).is_some_and(|v| words.iter().any(|w| v.trim().eq_ignore_ascii_case(w)));
    match status {
        403 if is("cf-mitigated", &["challenge"]) => Some("cloudflare"),
        429 if is("x-vercel-mitigated", &["challenge"]) => Some("vercel"),
        202 | 405 if is("x-amzn-waf-action", &["captcha", "challenge"]) => Some("aws-waf"),
        _ => None,
    }
}

/// The name a status is known by, for the common ones.
fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        429 => "Too Many Requests",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// The fields an answer gets for a document that is not the page asked
/// for, and the note that says what to do; None for a page that is.
pub fn report(status: u16, challenge: Option<&str>) -> Option<(Value, String)> {
    let named = |s: u16| match reason(s) {
        "" => s.to_string(),
        r => format!("{s} {r}"),
    };
    if let Some(who) = challenge {
        let by = match who {
            "cloudflare" => "Cloudflare",
            "vercel" => "Vercel",
            "aws-waf" => "AWS WAF",
            other => other,
        };
        let mut fields = json!({ "challenge": who });
        if status != 0 {
            fields["status"] = json!(status);
        }
        return Some((
            fields,
            format!(
                "a bot challenge ({by}), not the site's page: it may pass by itself (`wait --ms 5000`, then `url`); \
                 if not, get this content another way"
            ),
        ));
    }
    let what = match status {
        401 | 403 => "this page needs a login or access this browser doesn't have",
        404 | 410 => "the page shown is its error page: check the URL",
        429 => "it is rate limiting this browser: wait, then `goto` the URL again",
        400..=499 => "the page shown is its error page: check the URL and the request",
        500..=599 => "it failed, and the page shown is its error page: `goto` the URL again later",
        _ => return None,
    };
    Some((json!({ "status": status }), format!("the server answered {}; {what}", named(status))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_read_parses_and_odd_values_read_as_unknown() {
        assert_eq!(
            Doc::parse(&json!([404, 1764245000123.5, null])),
            Some(Doc { status: 404, origin: 1764245000123.5, challenge: None })
        );
        assert_eq!(Doc::parse(&json!([0, 5.0, "cloudflare"])).unwrap().challenge, Some("cloudflare"));
        // Not a status: read as unknown, not trusted.
        assert_eq!(Doc::parse(&json!([70000, 5.0, null])).unwrap().status, 0);
        assert_eq!(Doc::parse(&json!(["x", 5.0, "other"])), Some(Doc { status: 0, origin: 5.0, challenge: None }));
        assert_eq!(Doc::parse(&Value::Null), None);
        assert_eq!(Doc::parse(&json!([200])), None);
    }

    #[test]
    fn a_challenge_is_read_from_its_headers_in_any_case() {
        let h = |k: &str, v: &str| json!({ "Content-Type": "text/html", k: v });
        assert_eq!(challenge_from_headers(403, &h("cf-mitigated", "challenge")), Some("cloudflare"));
        assert_eq!(challenge_from_headers(403, &h("CF-Mitigated", "Challenge")), Some("cloudflare"));
        assert_eq!(challenge_from_headers(429, &h("x-vercel-mitigated", "challenge")), Some("vercel"));
        assert_eq!(challenge_from_headers(202, &h("x-amzn-waf-action", "captcha")), Some("aws-waf"));
        assert_eq!(challenge_from_headers(405, &h("X-Amzn-Waf-Action", "challenge")), Some("aws-waf"));
        // The header alone, or the status alone, is not one.
        assert_eq!(challenge_from_headers(200, &h("cf-mitigated", "challenge")), None);
        assert_eq!(challenge_from_headers(403, &h("server", "cloudflare")), None);
        assert_eq!(challenge_from_headers(429, &h("x-vercel-mitigated", "deny")), None);
        assert_eq!(challenge_from_headers(403, &Value::Null), None);
    }

    #[test]
    fn only_a_page_that_is_not_the_one_asked_for_is_reported() {
        for ok in [0, 200, 201, 204, 301, 304] {
            assert_eq!(report(ok, None), None, "{ok}");
        }
        let (fields, note) = report(404, None).unwrap();
        assert_eq!(fields, json!({ "status": 404 }));
        assert!(note.starts_with("the server answered 404 Not Found; ") && note.contains("check the URL"), "{note}");
        let (_, note) = report(503, None).unwrap();
        assert!(note.contains("503 Service Unavailable") && note.contains("again later"), "{note}");
        let (_, note) = report(418, None).unwrap();
        assert!(note.starts_with("the server answered 418; "), "{note}");
        let (fields, note) = report(403, Some("cloudflare")).unwrap();
        assert_eq!(fields, json!({ "challenge": "cloudflare", "status": 403 }));
        assert!(note.contains("bot challenge (Cloudflare)") && note.contains("another way"), "{note}");
        // A challenge whose status the engine didn't give.
        assert_eq!(report(0, Some("cloudflare")).unwrap().0, json!({ "challenge": "cloudflare" }));
    }

    #[test]
    fn more_is_asked_only_where_it_can_change_the_answer() {
        let d = |status, challenge| Doc { status, origin: 1.0, challenge };
        assert!(!d(200, None).wants_more());
        assert!(!d(301, None).wants_more());
        assert!(d(0, None).wants_more());
        assert!(d(202, None).wants_more());
        assert!(d(403, None).wants_more());
        assert!(!d(403, Some("cloudflare")).wants_more());
        // A challenge on a page whose status the engine didn't give.
        assert!(d(0, Some("cloudflare")).wants_more());
    }
}
