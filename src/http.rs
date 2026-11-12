//! HTTP outside the page: one request with the cookies a browser would
//! keep, for API tests and for setting up state a page then starts from.
//!
//! Native (ureq over rustls with ring), so a request costs no process
//! start: connections are pooled and kept alive, per host, for the life of
//! the process. Certificates are checked against the operating system's
//! store (its verifier on macOS and Windows; the system bundle, or
//! `SSL_CERT_FILE`, on Linux). Proxies come from the environment as curl
//! reads them: `https_proxy`/`HTTPS_PROXY` for https, `http_proxy` for http,
//! `all_proxy` for both, and `no_proxy` (names, `.suffix`, `*`, and CIDR
//! ranges) to bypass them.

use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    /// The last answer's headers.
    pub headers: Vec<(String, String)>,
    /// `Set-Cookie`s of every answer on the way, redirects included, each
    /// with the URL of the answer that set it.
    pub cookies: Vec<(String, String)>,
    pub body: String,
    /// The whole exchange, redirects included (ms).
    pub ms: f64,
}

/// What an `http` step sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// As given (`--json`, `--body`, a file's text).
    Raw(String),
    /// `--form name=value`…: `application/x-www-form-urlencoded`.
    Form(Vec<(String, String)>),
    /// `--multipart name=value` or `name=@file`…: `multipart/form-data`.
    Multipart(Vec<(String, String)>),
}

/// Whether to follow redirects, and to accept any certificate.
#[derive(Debug, Clone, Copy)]
pub struct How {
    pub follow: bool,
    pub insecure: bool,
}

/// Whether `Name: value` lines have one called `name` (any case).
pub fn has_header(headers: &[String], name: &str) -> bool {
    headers.iter().any(|h| h.split(':').next().is_some_and(|k| k.trim().eq_ignore_ascii_case(name)))
}

/// `application/x-www-form-urlencoded`, as a browser sends a form.
pub fn form_encoded(fields: &[(String, String)]) -> String {
    let enc = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
                b' ' => out.push('+'),
                b => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    };
    fields.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

/// `url` with `name=value` pairs added to its query, percent-encoded (a
/// `#fragment` stays last).
pub fn with_query(url: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return url.to_string();
    }
    let (rest, frag) = url.split_once('#').map_or((url, None), |(u, f)| (u, Some(f)));
    let enc = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
                b => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    };
    let q = pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    let sep = if !rest.contains('?') {
        "?"
    } else if rest.ends_with('?') || rest.ends_with('&') {
        ""
    } else {
        "&"
    };
    match frag {
        Some(f) => format!("{rest}{sep}{q}#{f}"),
        None => format!("{rest}{sep}{q}"),
    }
}

/// A `Location` against the URL it answered.
pub fn resolve(base: &str, loc: &str) -> String {
    if loc.contains("://") {
        return loc.to_string();
    }
    let Some(u) = Url::parse(base) else { return loc.to_string() };
    let origin = match u.port {
        Some(p) => format!("{}://{}:{p}", u.scheme, u.host_raw),
        None => format!("{}://{}", u.scheme, u.host_raw),
    };
    if let Some(rest) = loc.strip_prefix("//") {
        return format!("{}://{rest}", u.scheme);
    }
    if loc.starts_with('/') {
        return format!("{origin}{loc}");
    }
    if loc.starts_with('?') {
        return format!("{origin}{}{loc}", u.path);
    }
    let dir = &u.path[..u.path.rfind('/').map_or(0, |i| i + 1)];
    format!("{origin}{}{loc}", if dir.is_empty() { "/" } else { dir })
}

/// The parts of a URL a cookie cares about.
#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub scheme: String,
    /// Lowercase, without brackets or port.
    pub host: String,
    /// As written (an IPv6 address keeps its brackets).
    host_raw: String,
    pub port: Option<u16>,
    /// The path, without query or fragment (`/` when none).
    pub path: String,
}

impl Url {
    pub fn parse(url: &str) -> Option<Url> {
        let (scheme, rest) = url.split_once("://")?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host_raw, port) = if let Some(v6) = hostport.strip_prefix('[') {
            let close = v6.find(']')?;
            let port = v6[close + 1..].strip_prefix(':').and_then(|p| p.parse().ok());
            (format!("[{}]", &v6[..close]), port)
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
                    (h.to_string(), p.parse().ok())
                }
                _ => (hostport.to_string(), None),
            }
        };
        let path = tail.split(['?', '#']).next().unwrap_or("");
        Some(Url {
            scheme: scheme.to_ascii_lowercase(),
            host: host_raw.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase(),
            host_raw,
            port,
            path: if path.is_empty() { "/".into() } else { path.to_string() },
        })
    }
}

/// The most redirects followed, as curl's default and Chrome's.
const MAX_REDIRECTS: usize = 20;

/// The most of an answer read into memory.
const MAX_BODY: u64 = 256 << 20;

/// The client for checked certificates, and the one that accepts any.
fn agent(insecure: bool) -> &'static ureq::Agent {
    static AGENTS: OnceLock<[ureq::Agent; 2]> = OnceLock::new();
    &AGENTS.get_or_init(|| [build_agent(false), build_agent(true)])[insecure as usize]
}

fn build_agent(insecure: bool) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .root_certs(RootCerts::PlatformVerifier)
        .disable_verification(insecure)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        // Every answer is an answer, and redirects are followed here, so
        // the cookies set on the way are seen and sent on.
        .http_status_as_error(false)
        .max_redirects(0)
        .allow_non_standard_methods(true)
        // Chosen per request, by scheme (`proxy_for`).
        .proxy(None)
        .user_agent(concat!("navigera/", env!("CARGO_PKG_VERSION")))
        .accept("*/*")
        .max_idle_connections(64)
        .max_idle_connections_per_host(16)
        .build()
        .new_agent()
}

/// The proxy for `url`, as curl picks one from the environment
/// (`crate::proxy`).
fn proxy_for(url: &Url) -> Option<ureq::Proxy> {
    let (_, proxy) = crate::proxy::configured(url.scheme == "https", &url.host)?;
    ureq::Proxy::new(&proxy).ok()
}

/// A file's media type by its extension, for a multipart upload with no
/// `;type=`.
fn media_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// A quoted `Content-Disposition` parameter, as browsers escape one.
fn disposition_quoted(s: &str) -> String {
    s.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A")
}

/// One part of a multipart body: its name, the file name and media type
/// of an upload, and the bytes.
type Part = (String, Option<(String, String)>, Vec<u8>);

/// `multipart/form-data`: a part per field; `@path[;type=…][;filename=…]`
/// uploads that file. The boundary is one no part contains.
fn multipart(parts: &[(String, String)], boundary_seed: u64) -> Result<(Vec<u8>, String), String> {
    let mut fields: Vec<Part> = Vec::new();
    for (name, value) in parts {
        match value.strip_prefix('@') {
            Some(spec) => {
                let mut bits = spec.split(';');
                let path = bits.next().unwrap_or("").to_string();
                let (mut kind, mut file) = (None, None);
                for b in bits {
                    match b.split_once('=') {
                        Some(("type", t)) => kind = Some(t.trim().to_string()),
                        Some(("filename", f)) => file = Some(f.trim().trim_matches('"').to_string()),
                        _ => {}
                    }
                }
                let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                let file = file.unwrap_or_else(|| {
                    std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                let kind = kind.unwrap_or_else(|| media_type(&path).to_string());
                fields.push((name.clone(), Some((file, kind)), data));
            }
            None => fields.push((name.clone(), None, value.clone().into_bytes())),
        }
    }
    let mut seed = boundary_seed;
    let boundary = loop {
        let b = format!("------------------------{seed:016x}");
        if !fields.iter().any(|(_, _, d)| d.windows(b.len()).any(|w| w == b.as_bytes())) {
            break b;
        }
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    };
    let mut out = Vec::new();
    for (name, file, data) in &fields {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = disposition_quoted(name);
        match file {
            Some((f, kind)) => out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\nContent-Type: {kind}\r\n\r\n",
                    disposition_quoted(f)
                )
                .as_bytes(),
            ),
            None => {
                out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes())
            }
        }
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((out, format!("multipart/form-data; boundary={boundary}")))
}

/// The bytes a body sends, and the `Content-Type` it implies when the
/// request names none.
fn encoded(body: &Body) -> Result<(Vec<u8>, Option<String>), String> {
    Ok(match body {
        Body::Raw(b) => (b.clone().into_bytes(), None),
        Body::Form(fields) => {
            (form_encoded(fields).into_bytes(), Some("application/x-www-form-urlencoded".to_string()))
        }
        Body::Multipart(parts) => {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
                ^ (std::process::id() as u64) << 32;
            let (bytes, kind) = multipart(parts, seed)?;
            (bytes, Some(kind))
        }
    })
}

/// Make the request. `headers` are `Name: value` lines.
///
/// With `how.follow`, a 3xx with a `Location` is followed (at most
/// [`MAX_REDIRECTS`]): a 303, or a 301/302 after a POST, is fetched with GET
/// and no body, as browsers and curl do; a cookie set on the way is sent to
/// the later hops it matches; `Authorization` and `Cookie` lines the caller
/// gave stop at a change of host.
pub fn request(
    method: &str,
    url: &str,
    headers: &[String],
    body: Option<&Body>,
    limit: Duration,
    how: How,
) -> Result<Response, String> {
    let started = Instant::now();
    let fail = |at: &str, why: String| format!("{method} {at}: {why}");
    let mut payload = match body {
        Some(b) => {
            let (bytes, kind) = encoded(b).map_err(|e| fail(url, e))?;
            Some((bytes, kind.filter(|_| !has_header(headers, "content-type"))))
        }
        None => None,
    };
    let mut method = method.to_string();
    let mut at = url.to_string();
    let first_host = Url::parse(url).map(|u| (u.host, u.port));
    let mut cookies = Vec::new();
    let mut hop_jar = Jar::default();
    for hop in 0..=MAX_REDIRECTS {
        let u = Url::parse(&at).ok_or_else(|| fail(&at, "not a URL".into()))?;
        let left = limit.checked_sub(started.elapsed()).filter(|d| !d.is_zero());
        let Some(left) = left else {
            return Err(fail(&at, format!("timed out after {} ms", limit.as_millis())));
        };
        let same_host = first_host.as_ref().is_some_and(|h| *h == (u.host.clone(), u.port));
        let mut req = ureq::http::Request::builder().method(method.as_str()).uri(&at);
        let mut cookie_line: Option<String> = None;
        for h in headers {
            let Some((k, v)) = h.split_once(':') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if !same_host && (k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("cookie")) {
                continue;
            }
            if k.eq_ignore_ascii_case("cookie") {
                cookie_line = Some(match cookie_line {
                    Some(c) => format!("{c}; {v}"),
                    None => v.to_string(),
                });
                continue;
            }
            req = req.header(k, v);
        }
        let now = unix_now();
        if let Some(c) = hop_jar.header(&at, now) {
            cookie_line = Some(match cookie_line {
                Some(given) => format!("{given}; {c}"),
                None => c,
            });
        }
        if let Some(c) = cookie_line {
            req = req.header("Cookie", c);
        }
        let bytes = match &payload {
            Some((bytes, kind)) => {
                if let Some(kind) = kind {
                    req = req.header("Content-Type", kind.as_str());
                }
                bytes.clone()
            }
            None => Vec::new(),
        };
        let req = req.body(bytes).map_err(|e| fail(&at, e.to_string()))?;
        let a = agent(how.insecure);
        let req = a.configure_request(req).timeout_global(Some(left)).proxy(proxy_for(&u)).build();
        let resp = a.run(req).map_err(|e| {
            fail(
                &at,
                match e {
                    ureq::Error::Timeout(_) => format!("timed out after {} ms", limit.as_millis()),
                    e => {
                        let why = e.to_string();
                        if why.contains("invalid peer certificate") && !how.insecure {
                            format!(
                                "{why} (nothing this machine trusts vouches for the certificate; for a test \
                                 server's own, --insecure accepts it)"
                            )
                        } else {
                            why
                        }
                    }
                },
            )
        })?;
        let status = resp.status().as_u16();
        let got: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| (canonical(k.as_str()), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        for (_, v) in got.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")) {
            cookies.push((v.clone(), at.clone()));
            hop_jar.store(v, &at, now);
        }
        let location = got.iter().find(|(k, _)| k.eq_ignore_ascii_case("location")).map(|(_, v)| v.clone());
        let mut body = resp.into_body();
        if let (true, true, Some(loc)) = (how.follow, (300..400).contains(&status), location) {
            if hop == MAX_REDIRECTS {
                return Err(fail(url, format!("more than {MAX_REDIRECTS} redirects")));
            }
            // Read off, so the connection goes back to the pool.
            let _ = body.with_config().limit(MAX_BODY).reader().read_to_end(&mut Vec::new());
            if status == 303 && method != "HEAD" || matches!(status, 301 | 302) && method == "POST" {
                method = "GET".into();
                payload = None;
            }
            at = resolve(&at, &loc);
            continue;
        }
        let mut raw = Vec::new();
        if method != "HEAD" {
            body.with_config().limit(MAX_BODY).reader().read_to_end(&mut raw).map_err(|e| fail(&at, e.to_string()))?;
        }
        return Ok(Response {
            status,
            headers: got,
            cookies,
            body: String::from_utf8_lossy(&raw).into_owned(),
            ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    unreachable!("the loop returns by its last hop")
}

/// `content-type` as `Content-Type`: header names as servers write them.
fn canonical(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut up = true;
    for c in name.chars() {
        out.push(if up { c.to_ascii_uppercase() } else { c });
        up = c == '-';
    }
    out
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An HTTP date (`Wed, 21 Oct 2015 07:28:00 GMT`, or with dashes) in Unix
/// seconds.
fn http_date(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let parts: Vec<&str> = s.split([' ', '-', ':', ',']).filter(|p| !p.is_empty()).collect();
    // [weekday,] day month year h m s [GMT]
    let at = parts.iter().position(|p| p.chars().all(|c| c.is_ascii_digit()))?;
    let (d, mon, y, h, mi, se) = (
        parts.get(at)?,
        parts.get(at + 1)?,
        parts.get(at + 2)?,
        parts.get(at + 3)?,
        parts.get(at + 4)?,
        parts.get(at + 5)?,
    );
    let m = MONTHS.iter().position(|x| mon.to_ascii_lowercase().starts_with(x))? as i64 + 1;
    let mut y: i64 = y.parse().ok()?;
    if y < 100 {
        y += if y < 70 { 2000 } else { 1900 };
    }
    let days = days_from_civil(y, m, d.parse().ok()?);
    Some(days * 86_400 + h.parse::<i64>().ok()? * 3600 + mi.parse::<i64>().ok()? * 60 + se.parse::<i64>().ok()?)
}

/// A `Set-Cookie` header, read.
#[derive(Debug, Clone, PartialEq, Default)]
struct SetCookie {
    name: String,
    value: String,
    /// The `Domain` attribute, lowercase, without its leading dot.
    domain: Option<String>,
    path: Option<String>,
    /// When it expires (Unix s): `Max-Age` wins over `Expires`.
    until: Option<i64>,
    http_only: bool,
    secure: bool,
    same_site: Option<String>,
}

impl SetCookie {
    fn parse(set_cookie: &str, now: i64) -> Option<SetCookie> {
        let mut parts = set_cookie.split(';');
        let (name, value) = parts.next()?.split_once('=')?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let mut c = SetCookie { name, value: value.trim().to_string(), ..Default::default() };
        let (mut expires, mut max_age) = (None, None);
        for attr in parts {
            let (k, v) = attr.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((attr.trim(), ""));
            match k.to_ascii_lowercase().as_str() {
                "domain" if !v.is_empty() => c.domain = Some(v.trim_start_matches('.').to_ascii_lowercase()),
                "path" if v.starts_with('/') => c.path = Some(v.to_string()),
                "expires" => expires = http_date(v),
                "max-age" => max_age = v.parse::<i64>().ok(),
                "httponly" => c.http_only = true,
                "secure" => c.secure = true,
                "samesite" if !v.is_empty() => {
                    let mut s = v.to_ascii_lowercase();
                    s[..1].make_ascii_uppercase();
                    c.same_site = Some(s);
                }
                _ => {}
            }
        }
        c.until = max_age.map(|a| now + a).or(expires);
        Some(c)
    }

    /// Whether an answer from `u` may set (or expire) this cookie, by a
    /// browser's rules: its `Domain` is the answering host or a parent of
    /// it, never another site's, and `Secure` needs a secure origin. The
    /// browser's own cookie API checks neither: it stores what it is given.
    fn allowed_from(&self, u: &Url) -> bool {
        self.domain.as_ref().is_none_or(|d| domain_match(&u.host, d)) && (!self.secure || secure_context(u))
    }

    /// `Domain` as the browser's cookie API reads it: with a leading dot
    /// the cookie goes to subdomains too (what the attribute means; a
    /// public suffix the browser keeps to that one host itself), without
    /// one it is the host's alone. None for a host-only cookie: no
    /// `Domain`, or one naming an IP address or a single-label host.
    fn api_domain(&self, u: &Url) -> Option<String> {
        let d = self.domain.as_ref()?;
        (!is_ip(&u.host) && d.contains('.')).then(|| format!(".{d}"))
    }
}

/// A `Set-Cookie` value as navigera `cookies` words for the scenario's
/// browser: `set <name> <value> …`, or `delete <name> …` for one that
/// expires it.
pub fn cookie_words(set_cookie: &str, url: &str, now: i64) -> Option<Vec<String>> {
    let c = SetCookie::parse(set_cookie, now)?;
    let u = Url::parse(url)?;
    if !c.allowed_from(&u) {
        return None;
    }
    let domain = c.api_domain(&u);
    let value = c.value.trim_matches('"').to_string();
    let mut words = match c.until {
        Some(t) if t <= now => vec!["cookies".to_string(), "delete".into(), c.name],
        _ => vec!["cookies".to_string(), "set".into(), c.name, value],
    };
    match domain {
        Some(d) => words.extend(["--domain".to_string(), d]),
        None => words.extend(["--url".to_string(), url.to_string()]),
    }
    if words[1] == "set" {
        if let Some(p) = c.path {
            words.extend(["--path".to_string(), p]);
        }
        if let Some(t) = c.until {
            words.extend(["--expires".to_string(), t.to_string()]);
        }
        if c.http_only {
            words.push("--http-only".to_string());
        }
        if c.secure {
            words.push("--secure".to_string());
        }
        if let Some(s) = c.same_site {
            words.extend(["--same-site".to_string(), s]);
        }
    }
    Some(words)
}

/// A `Set-Cookie` value answered for `url` as the browser's own cookie:
/// `Network.setCookie` parameters, or `Network.deleteCookies` ones for a
/// cookie it expires (`true`); and the cookie's name. None for one a
/// browser would not take from that URL (another site's `Domain`, `Secure`
/// over plain http).
pub fn cdp_cookie(set_cookie: &str, url: &str, now: i64) -> Option<(bool, String, serde_json::Value)> {
    let c = SetCookie::parse(set_cookie, now)?;
    let u = Url::parse(url)?;
    if !c.allowed_from(&u) {
        return None;
    }
    let domain = c.api_domain(&u);
    let mut p = serde_json::json!({ "name": c.name });
    match &domain {
        Some(d) => p["domain"] = serde_json::json!(d),
        None => p["url"] = serde_json::json!(url),
    }
    if let Some(path) = &c.path {
        p["path"] = serde_json::json!(path);
    } else if domain.is_some() {
        p["path"] = serde_json::json!("/");
    }
    if c.until.is_some_and(|t| t <= now) {
        return Some((true, c.name, p));
    }
    p["value"] = serde_json::json!(c.value.trim_matches('"'));
    if let Some(t) = c.until {
        p["expires"] = serde_json::json!(t as f64);
    }
    if c.http_only {
        p["httpOnly"] = serde_json::json!(true);
    }
    if c.secure {
        p["secure"] = serde_json::json!(true);
    }
    if let Some(ss) = &c.same_site {
        let ss = match ss.to_ascii_lowercase().as_str() {
            "strict" => "Strict",
            "none" => "None",
            _ => "Lax",
        };
        p["sameSite"] = serde_json::json!(ss);
    }
    Some((false, c.name, p))
}

/// One cookie a scenario without a browser holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lowercase, no leading dot.
    pub domain: String,
    /// Set without a `Domain`: sent to that host only, not its subdomains.
    pub host_only: bool,
    pub path: String,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

/// The cookies of a scenario that runs with no browser: what its `http`
/// answers set, sent back with its later requests the way a browser would
/// (RFC 6265: domain, path, `Secure`, expiry), and saved in navigera's
/// state format (`state save`) so a browser can load them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Jar {
    cookies: Vec<Cookie>,
}

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

/// A secure context, as Chrome counts one for cookies: https, or this machine.
fn secure_context(u: &Url) -> bool {
    u.scheme == "https"
        || matches!(u.host.as_str(), "localhost" | "127.0.0.1" | "::1")
        || u.host.ends_with(".localhost")
}

fn domain_match(host: &str, domain: &str) -> bool {
    host == domain || (host.ends_with(domain) && host[..host.len() - domain.len()].ends_with('.') && !is_ip(host))
}

fn path_match(req: &str, cookie: &str) -> bool {
    req == cookie || (req.starts_with(cookie) && (cookie.ends_with('/') || req[cookie.len()..].starts_with('/')))
}

/// RFC 6265's default path: the request path up to its last `/`.
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => path[..i].to_string(),
    }
}

impl Jar {
    /// Take a `Set-Cookie` the answer from `url` sent.
    pub fn store(&mut self, set_cookie: &str, url: &str, now: i64) {
        let (Some(c), Some(u)) = (SetCookie::parse(set_cookie, now), Url::parse(url)) else { return };
        let (domain, host_only) = match c.domain {
            // A domain it may not set (another site's, or a parent of an IP).
            Some(d) if !domain_match(&u.host, &d) => return,
            Some(d) => (d, false),
            None => (u.host.clone(), true),
        };
        if c.secure && !secure_context(&u) {
            return;
        }
        let path = c.path.unwrap_or_else(|| default_path(&u.path));
        self.cookies.retain(|k| !(k.name == c.name && k.domain == domain && k.path == path));
        if c.until.is_some_and(|t| t <= now) {
            return;
        }
        self.cookies.push(Cookie {
            name: c.name,
            value: c.value,
            domain,
            host_only,
            path,
            expires: c.until,
            secure: c.secure,
            http_only: c.http_only,
            same_site: c.same_site,
        });
    }

    /// The cookies a request to `url` carries, longest path first.
    pub fn matching(&self, url: &str, now: i64) -> Vec<&Cookie> {
        let Some(u) = Url::parse(url) else { return Vec::new() };
        let mut out: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|t| t > now))
            .filter(|c| if c.host_only { u.host == c.domain } else { domain_match(&u.host, &c.domain) })
            .filter(|c| path_match(&u.path, &c.path))
            .filter(|c| !c.secure || secure_context(&u))
            .collect();
        // Stable: equal paths keep the order they were set in.
        out.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        out
    }

    /// The `Cookie` header for a request to `url`, if any cookie goes.
    pub fn header(&self, url: &str, now: i64) -> Option<String> {
        let pairs: Vec<String> = self.matching(url, now).iter().map(|c| format!("{}={}", c.name, c.value)).collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }

    /// The latest cookie of that name that hasn't expired, any domain.
    pub fn get(&self, name: &str, now: i64) -> Option<&Cookie> {
        self.cookies.iter().rev().find(|c| c.name == name && c.expires.is_none_or(|t| t > now))
    }

    /// As navigera's `state save` writes it (`{cookies, origins}`), so a
    /// browser scenario can start from it.
    pub fn state(&self, now: i64) -> serde_json::Value {
        let cookies: Vec<serde_json::Value> = self
            .cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|t| t > now))
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "value": c.value,
                    "domain": if c.host_only { c.domain.clone() } else { format!(".{}", c.domain) },
                    "path": c.path,
                    "expires": c.expires.map_or(-1.0, |t| t as f64),
                    "httpOnly": c.http_only,
                    "secure": c.secure,
                    "sameSite": c.same_site.clone().unwrap_or_else(|| "Lax".into()),
                })
            })
            .collect();
        serde_json::json!({ "cookies": cookies, "origins": [] })
    }

    /// The cookies of a `state save` file (a browser's or a jar's).
    pub fn load(&mut self, state: &serde_json::Value, now: i64) {
        for c in state.get("cookies").and_then(|c| c.as_array()).into_iter().flatten() {
            let (Some(name), Some(value), Some(domain)) =
                (c["name"].as_str(), c["value"].as_str(), c["domain"].as_str())
            else {
                continue;
            };
            let expires = c["expires"].as_f64().filter(|e| *e > 0.0).map(|e| e as i64);
            if expires.is_some_and(|t| t <= now) {
                continue;
            }
            let host_only = !domain.starts_with('.');
            let domain = domain.trim_start_matches('.').to_ascii_lowercase();
            let path = c["path"].as_str().unwrap_or("/").to_string();
            self.cookies.retain(|k| !(k.name == name && k.domain == domain && k.path == path));
            self.cookies.push(Cookie {
                name: name.to_string(),
                value: value.to_string(),
                domain,
                host_only,
                path,
                expires,
                secure: c["secure"].as_bool().unwrap_or(false),
                http_only: c["httpOnly"].as_bool().unwrap_or(false),
                same_site: c["sameSite"].as_str().map(String::from),
            });
        }
    }
}

/// Unix seconds now.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_becomes_navigera_words() {
        let now = 1_700_000_000;
        assert_eq!(
            cookie_words("sid=abc; Path=/; HttpOnly; SameSite=lax", "http://h/api", now).unwrap().join(" "),
            "cookies set sid abc --url http://h/api --path / --http-only --same-site Lax"
        );
        assert_eq!(
            cookie_words("t=1; Domain=.shop.test; Max-Age=60; Secure", "https://shop.test/", now).unwrap().join(" "),
            format!("cookies set t 1 --domain .shop.test --expires {} --secure", now + 60)
        );
        assert_eq!(
            cookie_words("sid=; Expires=Thu, 01 Jan 1970 00:00:00 GMT", "http://h/", now).unwrap().join(" "),
            "cookies delete sid --url http://h/"
        );
        assert_eq!(http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480));
        assert_eq!(http_date("Wed, 21-Oct-15 07:28:00 GMT"), Some(1_445_412_480));
        assert!(cookie_words("novalue", "http://h/", now).is_none());
    }

    #[test]
    fn redirects_resolve_against_the_url_they_answered() {
        assert_eq!(resolve("http://h:8/a/b?q=1", "/c"), "http://h:8/c");
        assert_eq!(resolve("http://h/a/b", "c?x"), "http://h/a/c?x");
        assert_eq!(resolve("https://h/a", "//o.test/z"), "https://o.test/z");
        assert_eq!(resolve("http://h/a", "https://o.test/"), "https://o.test/");
        assert_eq!(resolve("http://[::1]:9/a/b", "?p=2"), "http://[::1]:9/a/b?p=2");
        let u = Url::parse("HTTPS://User@Shop.Test:8443/api/v1?x#y").unwrap();
        assert_eq!(
            (u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str()),
            ("https", "shop.test", Some(8443), "/api/v1")
        );
        assert_eq!(Url::parse("http://h").unwrap().path, "/");
    }

    #[test]
    fn forms_and_queries_are_encoded_as_browsers_do() {
        let f = |p: &[(&str, &str)]| p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(form_encoded(&f(&[("email", "a+b@c.d"), ("note", "x y&z")])), "email=a%2Bb%40c.d&note=x+y%26z");
        assert_eq!(with_query("http://h/s", &f(&[("q", "a b"), ("n", "1")])), "http://h/s?q=a%20b&n=1");
        assert_eq!(with_query("http://h/s?x=1#top", &f(&[("q", "é")])), "http://h/s?x=1&q=%C3%A9#top");
        assert_eq!(with_query("http://h/s", &[]), "http://h/s");
        let (bytes, kind) = encoded(&Body::Form(f(&[("a", "1 2")]))).unwrap();
        assert_eq!((bytes.as_slice(), kind.as_deref()), (&b"a=1+2"[..], Some("application/x-www-form-urlencoded")));
        assert_eq!(encoded(&Body::Raw("{}".into())).unwrap(), (b"{}".to_vec(), None));
    }

    #[test]
    fn multipart_sends_fields_and_files_as_browsers_do() {
        let dir = std::env::temp_dir().join(format!("navigera-multipart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        std::fs::write(&file, b"PNG!").unwrap();
        let f = file.display().to_string();
        let parts = [
            ("note".to_string(), "x \"y\"".to_string()),
            ("pic".to_string(), format!("@{f}")),
            ("doc".to_string(), format!("@{f};type=text/plain;filename=r.txt")),
        ];
        let (bytes, kind) = multipart(&parts, 7).unwrap();
        let b = "------------------------0000000000000007";
        assert_eq!(kind, format!("multipart/form-data; boundary={b}"));
        let want = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nx \"y\"\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"pic\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG!\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"r.txt\"\r\nContent-Type: text/plain\r\n\r\nPNG!\r\n\
             --{b}--\r\n"
        );
        assert_eq!(String::from_utf8(bytes).unwrap(), want);
        // A boundary a part contains is never used.
        let parts = [("t".to_string(), b.to_string())];
        let (_, kind) = multipart(&parts, 7).unwrap();
        assert!(!kind.ends_with(b), "{kind}");
        assert!(multipart(&[("x".into(), "@/no/such/file".into())], 1).unwrap_err().starts_with("/no/such/file: "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_cookie_becomes_the_browsers_own_cookie() {
        let now = 1_000_000;
        let (delete, name, p) =
            cdp_cookie("sid=abc; Path=/app; HttpOnly; Secure; SameSite=None; Max-Age=60", "https://a.test/app/x", now)
                .unwrap();
        assert_eq!((delete, name.as_str()), (false, "sid"));
        assert_eq!(
            p,
            serde_json::json!({"name": "sid", "value": "abc", "url": "https://a.test/app/x", "path": "/app",
                "expires": (now + 60) as f64, "httpOnly": true, "secure": true, "sameSite": "None"})
        );
        let (_, _, p) = cdp_cookie("t=\"q\"; Domain=.a.test; SameSite=strict", "https://www.a.test/", now).unwrap();
        assert_eq!(
            p,
            serde_json::json!({"name": "t", "value": "q", "domain": ".a.test", "path": "/", "sameSite": "Strict"})
        );
        // Another site's cookie, a sibling's, and a Secure one over plain
        // http: a browser takes none of them, and neither does this.
        for (set, from) in [
            ("sid=x; Domain=bank.test", "https://evil.test/"),
            ("sid=x; Domain=b.a.test", "https://c.a.test/"),
            ("sid=x; Domain=evil.test", "https://notevil.test/"),
            ("sid=; Max-Age=0; Domain=bank.test", "https://evil.test/"),
            ("sid=x; Secure", "http://a.test/"),
        ] {
            assert!(cdp_cookie(set, from, now).is_none(), "{set} from {from}");
            assert!(cookie_words(set, from, now).is_none(), "{set} from {from}");
        }
        // A Domain naming the host itself on an IP or a bare name: host-only.
        let (_, _, p) = cdp_cookie("a=1; Domain=127.0.0.1", "http://127.0.0.1:8000/x", now).unwrap();
        assert_eq!(p, serde_json::json!({"name": "a", "value": "1", "url": "http://127.0.0.1:8000/x"}));
        let words = cookie_words("t=q; Domain=a.test", "https://www.a.test/", now).unwrap();
        assert_eq!(words, ["cookies", "set", "t", "q", "--domain", ".a.test"]);
        let (delete, _, p) = cdp_cookie("sid=; Max-Age=0", "https://a.test/", now).unwrap();
        assert!(delete && p.get("value").is_none(), "{p}");
        assert!(cdp_cookie("no equals sign", "https://a.test/", now).is_none());
    }

    #[test]
    fn header_names_are_written_canonically() {
        assert_eq!(canonical("content-type"), "Content-Type");
        assert_eq!(canonical("x-request-id"), "X-Request-Id");
    }

    /// A server that answers each request on a connection from `answers`,
    /// in turn, and records what it was sent and on which connection.
    fn serve(answers: Vec<String>) -> (String, std::sync::mpsc::Receiver<(usize, String)>) {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut answers = answers.into_iter();
            for (conn, s) in l.incoming().enumerate() {
                let Ok(s) = s else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut w = s;
                loop {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    if head.is_empty() {
                        break;
                    }
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0usize);
                    let mut body = vec![0; len];
                    std::io::Read::read_exact(&mut r, &mut body).unwrap();
                    head.push_str(&String::from_utf8_lossy(&body));
                    let _ = tx.send((conn, head));
                    let Some(a) = answers.next() else { return };
                    w.write_all(a.as_bytes()).unwrap();
                }
            }
        });
        (base, rx)
    }

    #[test]
    fn requests_follow_redirects_carry_cookies_and_reuse_the_connection() {
        let ok =
            |extra: &str, body: &str| format!("HTTP/1.1 200 OK\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len());
        let (base, seen) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /home\r\nSet-Cookie: sid=1; Path=/\r\nContent-Length: 3\r\n\r\nbye".into(),
            ok("set-cookie: seen=1\r\ncontent-type: text/plain\r\n", "home"),
            "HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope".into(),
            "HTTP/1.1 303 See Other\r\nLocation: /done\r\nContent-Length: 0\r\n\r\n".into(),
            ok("", "done"),
        ]);
        let how = How { follow: true, insecure: false };
        let t = Duration::from_secs(5);
        let r = request("POST", &format!("{base}/login"), &["X-A: 1".into()], Some(&Body::Raw("hi".into())), t, how)
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (200, "home"));
        assert!(r.headers.contains(&("Content-Type".into(), "text/plain".into())), "{:?}", r.headers);
        let set: Vec<(&str, String)> = r.cookies.iter().map(|(c, u)| (c.as_str(), u.replace(&base, ""))).collect();
        assert_eq!(set, [("sid=1; Path=/", "/login".to_string()), ("seen=1", "/home".to_string())]);
        let (c1, first) = seen.recv().unwrap();
        assert!(first.starts_with("POST /login HTTP/1.1\r\n") && first.contains("x-a: 1\r\n") && first.ends_with("hi"));
        // A 302 after a POST: GET, no body, the cookie it set going along.
        let (c2, second) = seen.recv().unwrap();
        assert!(second.starts_with("GET /home HTTP/1.1\r\n") && second.contains("cookie: sid=1\r\n"), "{second}");
        assert!(!second.contains("hi"), "{second}");
        // An error status is an answer; the same connection carries it.
        let r = request("GET", &format!("{base}/x"), &[], None, t, how).unwrap();
        assert_eq!((r.status, r.body.as_str()), (404, "nope"));
        let (c3, _) = seen.recv().unwrap();
        // Not followed: the 3xx itself.
        let r = request("PUT", &format!("{base}/y"), &[], None, t, How { follow: false, insecure: false }).unwrap();
        assert_eq!(r.status, 303);
        let (c4, _) = seen.recv().unwrap();
        assert_eq!((c1, c2, c3, c4), (0, 0, 0, 0), "one kept-alive connection for every request");
        assert!(r.ms > 0.0 && r.ms < 5000.0);
    }

    #[test]
    fn a_request_past_its_time_fails_saying_so() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/slow", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let _held: Vec<_> = l.incoming().take(1).collect();
            std::thread::sleep(Duration::from_secs(3));
        });
        let how = How { follow: true, insecure: false };
        let e = request("GET", &url, &[], None, Duration::from_millis(200), how).unwrap_err();
        assert_eq!(e, format!("GET {url}: timed out after 200 ms"));
        assert!(request("GET", "http://127.0.0.1:1/", &[], None, Duration::from_secs(2), how).is_err());
    }

    #[test]
    fn the_jar_sends_back_what_a_browser_would() {
        let now = 1_700_000_000;
        let mut jar = Jar::default();
        jar.store("sid=abc; Path=/; HttpOnly", "http://shop.test/login", now);
        jar.store("theme=dark", "http://shop.test/account/settings", now);
        jar.store("wide=1; Domain=.shop.test", "http://www.shop.test/", now);
        jar.store("evil=1; Domain=other.test", "http://shop.test/", now);
        jar.store("sec=1; Secure", "http://shop.test/", now);
        jar.store("lsec=1; Secure", "http://localhost:8000/", now);
        assert_eq!(jar.header("http://shop.test/", now).as_deref(), Some("sid=abc; wide=1"));
        // The default path is the request's directory, and a longer path goes first.
        assert_eq!(
            jar.header("http://shop.test/account/settings/x", now).as_deref(),
            Some("theme=dark; sid=abc; wide=1")
        );
        assert_eq!(jar.header("http://shop.test/accounts", now).as_deref(), Some("sid=abc; wide=1"));
        // Host-only stays on its host; a domain cookie reaches subdomains.
        assert_eq!(jar.header("http://api.shop.test/", now).as_deref(), Some("wide=1"));
        assert_eq!(jar.header("http://localhost:8000/", now).as_deref(), Some("lsec=1"));
        assert!(jar.header("http://other.test/", now).is_none());
        // Set again: replaced; expired: gone.
        jar.store("sid=new; Path=/", "http://shop.test/", now);
        assert_eq!(jar.get("sid", now).map(|c| c.value.as_str()), Some("new"));
        jar.store("sid=; Max-Age=0; Path=/", "http://shop.test/", now);
        assert!(jar.get("sid", now).is_none());
        jar.store("short=1; Max-Age=5", "http://shop.test/", now);
        assert!(jar.get("short", now + 10).is_none());
        assert!(jar.header("http://shop.test/", now + 10).as_deref() == Some("wide=1"));
    }

    #[test]
    fn a_jar_round_trips_through_navigeras_state_format() {
        let now = 1_700_000_000;
        let mut jar = Jar::default();
        jar.store("sid=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=60", "https://shop.test/a", now);
        jar.store("wide=1; Domain=shop.test", "https://shop.test/", now);
        let state = jar.state(now);
        assert_eq!(state["cookies"][0]["domain"], "shop.test");
        assert_eq!(state["cookies"][0]["expires"], (now + 60) as f64);
        assert_eq!(state["cookies"][0]["sameSite"], "Strict");
        assert_eq!(state["cookies"][1]["domain"], ".shop.test");
        assert_eq!(state["cookies"][1]["expires"], -1.0);
        let mut back = Jar::default();
        back.load(&state, now);
        assert_eq!(back.state(now), state);
        assert_eq!(back.header("https://shop.test/a/b", now), jar.header("https://shop.test/a/b", now));
        // A browser's own state file (an expired cookie left out).
        let browser = serde_json::json!({"cookies": [
            {"name": "a", "value": "1", "domain": "localhost", "path": "/", "expires": -1},
            {"name": "old", "value": "1", "domain": "localhost", "path": "/", "expires": 5.0}
        ], "origins": [{"origin": "http://localhost", "localStorage": []}]});
        let mut j = Jar::default();
        j.load(&browser, now);
        assert_eq!(j.header("http://localhost:3000/x", now).as_deref(), Some("a=1"));
    }
}
