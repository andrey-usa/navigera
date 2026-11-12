//! Proxies from the environment, read as curl reads them, for the
//! connections navigera opens itself: a `wss://` DevTools endpoint goes
//! through `https_proxy`/`HTTPS_PROXY` (or `all_proxy`) as an HTTP CONNECT
//! tunnel, unless `no_proxy` names its host; the `http` feature's requests
//! use the same rules. Credentials in the proxy's URL
//! (`http://user:password@host:port`, percent-encoded) are sent as Basic
//! auth and never shown: messages name the proxy by host and port.

use anyhow::{anyhow, bail, Result};

/// The proxy curl would use for `host` over https (`secure`) or http, by
/// the variable that set it: `(variable, url)`.
pub fn configured(secure: bool, host: &str) -> Option<(&'static str, String)> {
    let names: &[&'static str] = if secure {
        &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else {
        // curl ignores HTTP_PROXY in capitals (a CGI server sets it from a header).
        &["http_proxy", "all_proxy", "ALL_PROXY"]
    };
    let (name, url) = names
        .iter()
        .find_map(|n| std::env::var(n).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(|v| (*n, v)))?;
    let skip = ["no_proxy", "NO_PROXY"].iter().find_map(|n| std::env::var(n).ok()).unwrap_or_default();
    if no_proxy(&skip, host) {
        return None;
    }
    Some((name, url))
}

/// Whether `host` is in a `no_proxy` list: `*`, a name (its subdomains
/// too), `.suffix`, an address, or an address range (`10.0.0.0/8`).
pub fn no_proxy(list: &str, host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let ip: Option<std::net::IpAddr> = host.parse().ok();
    list.split(',').map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        if e == "*" {
            return true;
        }
        if let (Some(ip), Some((net, bits))) = (ip, e.split_once('/')) {
            let net = net.trim_start_matches('[').trim_end_matches(']');
            let (Ok(net), Ok(bits)) = (net.parse::<std::net::IpAddr>(), bits.parse::<u32>()) else { return false };
            return in_range(ip, net, bits);
        }
        let e = e.trim_start_matches("*.").trim_start_matches('.');
        let e = e.trim_start_matches('[').trim_end_matches(']');
        host == e || (host.ends_with(e) && host[..host.len() - e.len()].ends_with('.'))
    })
}

fn in_range(ip: std::net::IpAddr, net: std::net::IpAddr, bits: u32) -> bool {
    use std::net::IpAddr::{V4, V6};
    match (ip, net) {
        (V4(a), V4(n)) if bits <= 32 => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (V6(a), V6(n)) if bits <= 128 => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// An HTTP proxy that tunnels with CONNECT.
#[derive(Debug, Clone, PartialEq)]
pub struct Proxy {
    /// As written (an IPv6 address keeps its brackets).
    pub host: String,
    pub port: u16,
    /// `user:password`, decoded, then base64 for `Proxy-Authorization`.
    auth: Option<String>,
    /// The variable it came from, for messages.
    from: &'static str,
}

impl Proxy {
    /// The proxy for a `wss://` endpoint on `host`, if the environment
    /// names one; a proxy navigera can't use is an error, not a silent
    /// direct connection.
    pub fn for_secure(host: &str) -> Result<Option<Proxy>> {
        match configured(true, host) {
            Some((from, url)) => Proxy::parse(&url, from).map(Some),
            None => Ok(None),
        }
    }

    /// `http://[user:password@]host[:port][/]` or `host[:port]`; curl's
    /// default port, 1080.
    pub fn parse(url: &str, from: &'static str) -> Result<Proxy> {
        let rest = match url.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => bail!(
                "${from}: a proxy reached over TLS (https://) is not supported for the DevTools connection; an http:// proxy (one that tunnels with CONNECT) is"
            ),
            Some((scheme, _)) => bail!(
                "${from}: a {scheme}:// proxy is not supported for the DevTools connection; an http:// proxy (one that tunnels with CONNECT) is"
            ),
            None => url,
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };
        let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
            let close = v6.find(']').ok_or_else(|| anyhow!("${from}: an IPv6 address without its closing ']'"))?;
            (format!("[{}]", &v6[..close]), v6[close + 1..].strip_prefix(':'))
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (hostport.to_string(), None),
            }
        };
        if host.is_empty() || host == "[]" {
            bail!("${from}: no proxy host in it");
        }
        let port = match port {
            Some(p) => p.parse::<u16>().map_err(|_| anyhow!("${from}: the proxy's port is not a number"))?,
            None => 1080,
        };
        let auth = userinfo.map(|u| {
            use base64::Engine as _;
            let (user, password) = u.split_once(':').unwrap_or((u, ""));
            let plain = format!("{}:{}", percent_decoded(user), percent_decoded(password));
            base64::engine::general_purpose::STANDARD.encode(plain)
        });
        Ok(Proxy { host, port, auth, from })
    }

    /// How messages name it: `host:port (from $HTTPS_PROXY)`, never its
    /// credentials.
    pub fn shown(&self) -> String {
        format!("{}:{} (from ${})", self.host, self.port, self.from)
    }

    /// A TCP connection through the proxy to `authority` (`host:port`), the
    /// proxy's answer read and checked, ready for TLS.
    pub async fn tunnel(&self, authority: &str) -> Result<tokio::net::TcpStream> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let host = self.host.trim_start_matches('[').trim_end_matches(']');
        let mut stream = tokio::net::TcpStream::connect((host, self.port))
            .await
            .map_err(|e| anyhow!("the proxy {}: {e}", self.shown()))?;
        let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
        if let Some(auth) = &self.auth {
            request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.map_err(|e| anyhow!("the proxy {}: {e}", self.shown()))?;
        // The answer's head, a byte at a time: what follows it is the
        // endpoint's, for TLS (the client speaks first, so nothing is lost).
        let mut head = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= 16 << 10 {
                bail!("the proxy {} answered CONNECT with a head over 16 KB", self.shown());
            }
            match stream.read(&mut byte).await {
                Ok(0) => bail!("the proxy {} closed the connection before answering CONNECT {authority}", self.shown()),
                Ok(_) => head.push(byte[0]),
                Err(e) => bail!("the proxy {}: {e}", self.shown()),
            }
        }
        let head = String::from_utf8_lossy(&head);
        let status_line = head.lines().next().unwrap_or("").trim();
        let code = status_line.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
        match code {
            200..=299 => Ok(stream),
            407 if self.auth.is_some() => bail!(
                "the proxy {} turned down the credentials in its URL ({status_line})",
                self.shown()
            ),
            407 => bail!(
                "the proxy {} wants credentials ({status_line}): put them in its URL, http://user:password@host:port (percent-encode an @ or : in them)",
                self.shown()
            ),
            _ => bail!("the proxy {} refused the tunnel to {authority}: {status_line}", self.shown()),
        }
    }
}

/// `%XX` decoded, as curl reads a proxy URL's user and password.
fn percent_decoded(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_matches_as_curl_does() {
        let list = "localhost, .corp.test,api.example.com,10.0.0.0/8,::1,[fd00::]/8";
        assert!(no_proxy(list, "localhost"));
        assert!(no_proxy(list, "a.corp.test") && no_proxy(list, "corp.test"));
        assert!(no_proxy(list, "api.example.com") && no_proxy(list, "v2.api.example.com"));
        assert!(!no_proxy(list, "badapi.example.com") && !no_proxy(list, "example.com"));
        assert!(no_proxy(list, "10.1.2.3") && !no_proxy(list, "11.0.0.1"));
        assert!(no_proxy(list, "[::1]") && no_proxy(list, "fd12::1") && !no_proxy(list, "fe80::1"));
        assert!(no_proxy("*", "anything.test") && !no_proxy("", "h"));
    }

    #[test]
    fn a_proxy_url_is_read_as_curl_reads_it() {
        use base64::Engine as _;
        let p = Proxy::parse("http://ann:p%40ss%3Aw@proxy.corp.test:3128/", "HTTPS_PROXY").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.corp.test", 3128));
        let auth = base64::engine::general_purpose::STANDARD.decode(p.auth.clone().unwrap()).unwrap();
        assert_eq!(auth, b"ann:p@ss:w");
        assert_eq!(p.shown(), "proxy.corp.test:3128 (from $HTTPS_PROXY)");
        assert!(!p.shown().contains("ann") && !p.shown().contains("ss"));
        let bare = Proxy::parse("10.0.0.1", "all_proxy").unwrap();
        assert_eq!((bare.host.as_str(), bare.port, bare.auth.is_none()), ("10.0.0.1", 1080, true));
        let v6 = Proxy::parse("http://[fd00::1]:8080", "https_proxy").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("[fd00::1]", 8080));
        for (url, says) in [
            ("https://p:443", "over TLS"),
            ("socks5://p:1080", "socks5:// proxy is not supported"),
            ("http://p:port", "not a number"),
            ("http://", "no proxy host"),
        ] {
            let err = Proxy::parse(url, "HTTPS_PROXY").unwrap_err().to_string();
            assert!(err.contains(says) && err.starts_with("$HTTPS_PROXY"), "{url}: {err}");
        }
        assert_eq!(percent_decoded("a%2"), "a%2");
        assert_eq!(percent_decoded("%zz%41"), "%zzA");
    }

    /// A proxy that answers CONNECT with `answer`, then echoes what it gets.
    async fn proxy_answering(answer: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).await.unwrap() == 0 {
                    break;
                }
                head.push(b[0]);
            }
            s.write_all(answer.as_bytes()).await.unwrap();
            let mut echo = [0u8; 4];
            if s.read_exact(&mut echo).await.is_ok() {
                s.write_all(&echo).await.unwrap();
            }
            String::from_utf8_lossy(&head).into_owned()
        });
        (port, task)
    }

    #[tokio::test]
    async fn a_tunnel_sends_connect_with_the_credentials_and_hands_over_the_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (port, seen) = proxy_answering("HTTP/1.1 200 Connection established\r\n\r\n").await;
        let proxy = Proxy::parse(&format!("http://u:p%40w@127.0.0.1:{port}"), "HTTPS_PROXY").unwrap();
        let mut s = proxy.tunnel("cdp.example.test:443").await.unwrap();
        s.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        s.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"ping", "the bytes after the head reach the endpoint");
        let head = seen.await.unwrap();
        assert!(head.starts_with("CONNECT cdp.example.test:443 HTTP/1.1\r\nHost: cdp.example.test:443\r\n"), "{head}");
        assert!(head.contains("Proxy-Authorization: Basic dTpwQHc=\r\n"), "{head}");
    }

    #[tokio::test]
    async fn a_refused_tunnel_says_why_without_the_credentials() {
        for (answer, with_auth, says) in [
            ("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n", false, "wants credentials"),
            ("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n", true, "turned down the credentials"),
            (
                "HTTP/1.1 403 Forbidden\r\nX: y\r\n\r\n",
                true,
                "refused the tunnel to h.test:443: HTTP/1.1 403 Forbidden",
            ),
        ] {
            let (port, _seen) = proxy_answering(answer).await;
            let url =
                if with_auth { format!("http://ann:s3cret@127.0.0.1:{port}") } else { format!("127.0.0.1:{port}") };
            let err = Proxy::parse(&url, "HTTPS_PROXY").unwrap().tunnel("h.test:443").await.unwrap_err().to_string();
            assert!(err.contains(says) && !err.contains("s3cret") && !err.contains("ann"), "{err}");
        }
    }
}
