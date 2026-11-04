//! A session's domain allowlist (`start --allow-domains`, the
//! `NAVIGERA_ALLOW_DOMAINS` variable, MCP `--allow-domains`).
//!
//! The policy is a hostname policy, not an origin policy: it compares the
//! request URL's hostname, ignoring scheme and port. An exact entry
//! (`example.com`) allows that hostname only; a `*.example.com` entry
//! allows its proper subdomains, never `example.com` itself and never
//! `badexample.com`. A request whose hostname matches no entry is refused
//! at the Fetch request stage, before any `route` rule sees it, and on
//! every redirect hop (each hop pauses again at the request stage).
//!
//! `--allow-domains-docs-only` narrows the check to top-level navigation:
//! a request is checked only when it is a `Document` request for the tab's
//! main frame. That mode is a navigation restriction, not exfiltration
//! protection: subresources, beacons and script requests to other hosts
//! still go out.
//!
//! What this deliberately does not cover: non-network URL schemes
//! (`about:`, `blob:`, `data:`) carry no host to check and pass through,
//! and `file:` URLs are outside this policy (local file access is a
//! separate concern). WebSockets handshake through Fetch, WebRTC, DNS and
//! browser-origin background traffic are not proven covered by Fetch
//! interception; they are recorded as limits in the docs, not claimed.
//! Non-ASCII hostnames are rejected: entries must be ASCII (no IDNA
//! mapping is done, so a Unicode entry could not be matched reliably).

use std::net::IpAddr;

/// One allowlist entry: an exact hostname/IP, or a wildcard subdomain
/// suffix (the part after `*.`).
#[derive(Clone, Debug, PartialEq)]
pub enum DomainEntry {
    Exact(String),
    Wildcard(String),
}

impl DomainEntry {
    /// The entry as the user wrote it (normalized: lowercased, no trailing
    /// dot), for display in `status`.
    pub fn display(&self) -> String {
        match self {
            DomainEntry::Exact(host) => host.clone(),
            DomainEntry::Wildcard(suffix) => format!("*.{suffix}"),
        }
    }

    fn matches(&self, host: &str) -> bool {
        match self {
            DomainEntry::Exact(want) => host == want,
            DomainEntry::Wildcard(suffix) => {
                host.len() > suffix.len() + 1
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }
        }
    }
}

/// A parsed allowlist: the entries, and whether only top-level navigation
/// is checked.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainPolicy {
    pub entries: Vec<DomainEntry>,
    pub docs_only: bool,
}

impl DomainPolicy {
    /// Parse a comma- and/or whitespace-separated list. An empty list, a
    /// malformed entry, or an entry carrying userinfo, a path, a query, a
    /// port or a scheme is an error: a policy that cannot be read is
    /// refused rather than silently disabled.
    pub fn parse(list: &str, docs_only: bool) -> Result<DomainPolicy, String> {
        let mut entries = Vec::new();
        for raw in list.split(|c: char| c == ',' || c.is_whitespace()) {
            if raw.is_empty() {
                continue;
            }
            entries.push(parse_entry(raw)?);
        }
        if entries.is_empty() {
            return Err("--allow-domains needs at least one hostname, got an empty list".to_string());
        }
        Ok(DomainPolicy { entries, docs_only })
    }

    /// Does this hostname pass the policy?
    pub fn allows_host(&self, host: &str) -> bool {
        let host = normalize_host(host);
        !host.is_empty() && self.entries.iter().any(|e| e.matches(&host))
    }

    /// Does this request URL pass? URLs with no network host (about, blob,
    /// data, file and other non-hierarchical schemes) pass: there is no
    /// host to check. A URL that looks hierarchical but has no usable host
    /// does not pass.
    pub fn allows_url(&self, url: &str) -> bool {
        match url_host(url) {
            Host::None => true,
            Host::Missing => false,
            Host::Name(host) => self.allows_host(&host),
        }
    }

    /// The entries as written (normalized), for display.
    pub fn display_entries(&self) -> Vec<String> {
        self.entries.iter().map(DomainEntry::display).collect()
    }
}

enum Host {
    /// No host to check (a non-network scheme).
    None,
    /// A hierarchical URL whose host could not be read.
    Missing,
    Name(String),
}

fn parse_entry(raw: &str) -> Result<DomainEntry, String> {
    let bad = |why: &str| format!("--allow-domains: invalid entry {raw:?}: {why}");
    if !raw.is_ascii() {
        return Err(bad("non-ASCII hostnames are not supported; use the ASCII form"));
    }
    let (wildcard, name) = match raw.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    if name.is_empty() {
        return Err(bad("empty hostname"));
    }
    if raw.contains('/')
        || raw.contains('?')
        || raw.contains('#')
        || raw.contains('@')
        || raw.contains('[')
        || raw.contains(']')
    {
        return Err(bad("an entry is a hostname only, not a URL (no scheme, userinfo, path or query)"));
    }
    // An IP literal is an exact entry only; a port is never part of an
    // entry (the policy ignores ports, so `example.com:8080` would be a
    // confusing way to write `example.com`).
    if raw.contains(':') {
        return match raw.parse::<IpAddr>() {
            Ok(_) if !wildcard => Ok(DomainEntry::Exact(raw.to_ascii_lowercase())),
            Ok(_) => Err(bad("a wildcard cannot cover an IP literal")),
            Err(_) => Err(bad("a port is not part of an entry; the policy ignores ports")),
        };
    }
    if wildcard && name.parse::<IpAddr>().is_ok() {
        return Err(bad("a wildcard cannot cover an IP literal"));
    }
    if name.starts_with('.') || name.ends_with('.') {
        return Err(bad("a hostname has no leading or trailing dot"));
    }
    if name.len() > 253 {
        return Err(bad("hostname longer than 253 characters"));
    }
    for label in name.split('.') {
        if label.is_empty() {
            return Err(bad("empty label (two dots in a row)"));
        }
        if label.len() > 63 {
            return Err(bad("label longer than 63 characters"));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(bad("a label starts and ends with a letter or digit, not a hyphen"));
        }
        if !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(bad("a label holds letters, digits, hyphens and underscores only"));
        }
    }
    let host = normalize_host(name);
    Ok(if wildcard { DomainEntry::Wildcard(host) } else { DomainEntry::Exact(host) })
}

/// Lowercase, one trailing dot stripped (a fully-qualified name is the
/// same host).
fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The hostname of a URL, without its port. Hand-rolled: this crate has no
/// URL dependency, and the check needs only the host part. Returns
/// [`Host::None`] for schemes that carry no network host.
fn url_host(url: &str) -> Host {
    let Some((scheme, rest)) = url.split_once(':') else {
        return Host::Missing;
    };
    if !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        return Host::Missing;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "about" | "blob" | "data" | "javascript" | "chrome" | "devtools" | "file" => return Host::None,
        _ => {}
    }
    let Some(after) = rest.strip_prefix("//") else {
        return if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ws" | "wss") {
            Host::Missing
        } else {
            Host::None
        };
    };
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    if authority.is_empty() {
        return Host::Missing;
    }
    // IPv6 literal: `[::1]` or `[::1]:8080`.
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, _)) = bracketed.split_once(']') else {
            return Host::Missing;
        };
        return if host.is_empty() { Host::Missing } else { Host::Name(normalize_host(host)) };
    }
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        Host::Missing
    } else {
        Host::Name(normalize_host(host))
    }
}

/// The hostname of a URL for refusal messages ("" when there is none).
pub fn refusal_host(url: &str) -> String {
    match url_host(url) {
        Host::Name(host) => host,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(list: &str) -> DomainPolicy {
        DomainPolicy::parse(list, false).unwrap()
    }

    #[test]
    fn exact_matches_only_itself() {
        let p = policy("example.com");
        assert!(p.allows_url("https://example.com/"));
        assert!(p.allows_url("https://example.com:8443/x?q=1"));
        assert!(p.allows_url("HTTPS://EXAMPLE.COM./"));
        assert!(!p.allows_url("https://sub.example.com/"));
        assert!(!p.allows_url("https://badexample.com/"));
        assert!(!p.allows_url("https://example.com.evil.com/"));
    }

    #[test]
    fn wildcard_matches_proper_subdomains_only() {
        let p = policy("*.example.com");
        assert!(p.allows_url("https://a.example.com/"));
        assert!(p.allows_url("https://deep.a.example.com/x"));
        assert!(!p.allows_url("https://example.com/"));
        assert!(!p.allows_url("https://badexample.com/"));
        assert!(!p.allows_url("https://example.com.evil.com/"));
    }

    #[test]
    fn mixed_list_and_ip_entries() {
        let p = policy("example.com, *.cdn.example.org 127.0.0.1");
        assert!(p.allows_url("https://example.com/"));
        assert!(p.allows_url("http://a.cdn.example.org/f.js"));
        assert!(!p.allows_url("http://cdn.example.org/f.js"));
        assert!(p.allows_url("http://127.0.0.1:9000/"));
        assert!(!p.allows_url("http://127.0.0.2:9000/"));
        assert!(!p.allows_url("http://localhost:9000/"));
    }

    #[test]
    fn malformed_lists_are_errors_not_silent_disable() {
        for bad in [
            "",
            "   ",
            "example.com,",
            "https://example.com",
            "example.com/path",
            "user@example.com",
            "example.com?x=1",
            "example.com:8080",
            "*.127.0.0.1",
            "-example.com",
            "example..com",
            ".example.com",
            "bücher.de",
        ] {
            // "example.com," parses: a trailing separator is not an entry.
            if *bad == *"example.com," {
                continue;
            }
            assert!(DomainPolicy::parse(bad, false).is_err(), "{bad:?} must be rejected");
        }
        assert!(DomainPolicy::parse("example.com,", false).is_ok());
    }

    #[test]
    fn network_schemes_without_an_authority_fail_closed() {
        let p = policy("example.com");
        for url in [
            "http:",
            "https:example.com",
            "http:/example.com",
            "ws:socket",
            "wss:/socket",
            "HTTP:example.com",
        ] {
            assert!(!p.allows_url(url), "{url:?} has no network authority");
        }
        assert!(p.allows_url("custom:opaque"));
        assert!(p.allows_url("https://example.com/"));
    }

    #[test]
    fn non_network_schemes_pass_and_broken_urls_fail() {
        let p = policy("example.com");
        assert!(p.allows_url("about:blank"));
        assert!(p.allows_url("data:text/html,hi"));
        assert!(p.allows_url("blob:https://example.com/uuid"));
        assert!(!p.allows_url("not a url"));
        assert!(!p.allows_url("https:///path"));
    }
}
