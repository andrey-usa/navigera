//! `totp`: the current one-time code of an authenticator app (RFC 6238,
//! the codes Google Authenticator, 1Password and the like show), for a
//! sign-in's second step. The secret comes from an environment variable,
//! never from the command line: its base32 key, or the `otpauth://` URI
//! a site's QR code holds.
//!
//! SHA-1 and HMAC are written out here (RFC 3174, RFC 2104) rather than
//! pulled in: navigera's binary has no crypto dependency, and these are a
//! few dozen lines checked against the RFCs' own test vectors.

use anyhow::{bail, Result};

/// One code's settings: the key, how many digits, how long it lasts.
#[derive(Debug, Clone, PartialEq)]
pub struct Totp {
    pub key: Vec<u8>,
    pub digits: u32,
    pub period: u64,
}

impl Totp {
    /// A base32 key (spaces, dashes and case ignored) or an
    /// `otpauth://totp/...?secret=...&digits=...&period=...` URI.
    pub fn parse(raw: &str) -> Result<Totp> {
        let raw = raw.trim();
        let Some(query) = raw.strip_prefix("otpauth://") else {
            return Ok(Totp { key: base32(raw)?, digits: 6, period: 30 });
        };
        let Some((kind, query)) =
            query.split_once('/').map(|(k, rest)| (k, rest.split_once('?').map_or("", |(_, q)| q)))
        else {
            bail!("an otpauth URI reads otpauth://totp/<label>?secret=...");
        };
        if !kind.eq_ignore_ascii_case("totp") {
            bail!("otpauth://{kind}: only time-based codes (totp) are supported");
        }
        let mut t = Totp { key: Vec::new(), digits: 6, period: 30 };
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k.to_ascii_lowercase().as_str() {
                "secret" => t.key = base32(&v.replace("%20", ""))?,
                "digits" => {
                    t.digits = v
                        .parse()
                        .ok()
                        .filter(|d| (6..=10).contains(d))
                        .ok_or_else(|| anyhow::anyhow!("otpauth digits must be 6 to 10, got {v:?}"))?
                }
                "period" => {
                    t.period = v
                        .parse()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or_else(|| anyhow::anyhow!("otpauth period must be a number of seconds, got {v:?}"))?
                }
                "algorithm" if !v.eq_ignore_ascii_case("sha1") => {
                    bail!("otpauth algorithm {v}: only SHA1, which nearly every site uses, is supported")
                }
                _ => {}
            }
        }
        if t.key.is_empty() {
            bail!("the otpauth URI has no secret=");
        }
        Ok(t)
    }

    /// The code for `unix_s`, and how many seconds it stays valid.
    pub fn at(&self, unix_s: u64) -> (String, u64) {
        let step = unix_s / self.period;
        let mac = hmac_sha1(&self.key, &step.to_be_bytes());
        let off = (mac[19] & 0x0f) as usize;
        let n = u32::from_be_bytes([mac[off] & 0x7f, mac[off + 1], mac[off + 2], mac[off + 3]]);
        let code = n as u64 % 10u64.pow(self.digits);
        (format!("{code:0width$}", width = self.digits as usize), self.period - unix_s % self.period)
    }

    /// The code to type now: the next one when this one has under 3 s
    /// left, so it can't run out between reading it and the site checking
    /// it. Waits for that next one to start.
    pub fn now(&self) -> (String, u64) {
        let unix = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let (code, left) = self.at(unix());
        if left >= 3 || self.period <= 3 {
            return (code, left);
        }
        std::thread::sleep(std::time::Duration::from_secs(left));
        self.at(unix())
    }
}

/// The secret in the environment variable `name`.
pub fn from_env(name: &str) -> Result<Totp> {
    let Some(raw) = std::env::var_os(name) else {
        bail!("totp: no environment variable {name} (set it to the base32 key or the otpauth:// URI of the authenticator)");
    };
    Totp::parse(&raw.to_string_lossy()).map_err(|e| e.context(format!("totp: {name}")))
}

fn base32(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            // The offending character is part of the secret: not shown.
            _ => bail!("the key is not base32 (letters A-Z and digits 2-7)"),
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if out.is_empty() {
        bail!("the key is empty");
    }
    Ok(out)
}

fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut data = msg.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for block in data.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).chain(msg.iter().copied()).collect();
    let outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).chain(sha1(&inner)).collect();
    sha1(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_and_hmac_match_the_rfcs() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let long = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha1(long.as_bytes())), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        // RFC 2202 test cases 1 and 6 (a key longer than a block).
        assert_eq!(hex(&hmac_sha1(&[0x0b; 20], b"Hi There")), "b617318655057264e28bc0b6fb378c8ef146be00");
        assert_eq!(
            hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn codes_match_rfc_6238() {
        // The RFC's SHA1 key, "12345678901234567890", in base32.
        let t = Totp { digits: 8, ..Totp::parse("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap() };
        for (time, code) in [
            (59, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            assert_eq!(t.at(time).0, code, "at {time}");
        }
        assert_eq!(t.at(59).1, 1, "one second left of its 30");
    }

    #[test]
    fn secrets_read_as_keys_or_uris() {
        let key = Totp::parse("gezd gnbv-gy3t qojq").unwrap();
        assert_eq!((key.key.as_slice(), key.digits, key.period), (&b"1234567890"[..], 6, 30));
        let uri =
            Totp::parse("otpauth://totp/Shop:ada%40example.com?secret=GEZDGNBVGY3TQOJQ&issuer=Shop&digits=8&period=60")
                .unwrap();
        assert_eq!((uri.digits, uri.period), (8, 60));
        assert_eq!(uri.key, b"1234567890");
        assert!(Totp::parse("otpauth://hotp/x?secret=GEZD").unwrap_err().to_string().contains("only time-based"));
        assert!(Totp::parse("otpauth://totp/x?secret=GEZDGNBV&algorithm=SHA256").is_err());
        let bad = Totp::parse("JBSWY3DP!").unwrap_err().to_string();
        assert!(bad.contains("not base32") && !bad.contains('!'), "the secret's characters never show: {bad}");
        assert!(Totp::parse("otpauth://totp/x?issuer=Shop").is_err());
    }
}
