//! `passkey …`: a virtual WebAuthn authenticator (Chrome's `WebAuthn`
//! domain) in every tab, so a page's `navigator.credentials.create()` and
//! `.get()` are answered without a person, a phone or a security key.
//!
//! Chrome keeps an authenticator per tab. The session adds one to each tab
//! it has and to each tab opened later, which starts with the passkeys the
//! session's other tabs hold. A passkey leaves the browser in the shape
//! Playwright's `credentials.get()` gives (`id`, `rpId`, `userHandle`,
//! `privateKey`, `publicKey`, `signCount`, base64url), so `state save`
//! files carry them to either tool and back.

use std::collections::HashMap;

use anyhow::{bail, Result};
use base64::Engine as _;
use serde_json::{json, Value};

/// The session's authenticator: its options while passkeys are on, and
/// per tab (target id) the id Chrome gave it there.
#[derive(Debug, Default, Clone)]
pub struct Passkeys {
    pub options: Option<Value>,
    pub ids: HashMap<String, String>,
    /// `passkey verify off`: the person "fails" the fingerprint or PIN.
    pub unverified: bool,
}

/// `addVirtualAuthenticator` options: a platform authenticator (a phone's
/// or laptop's, `internal`) unless `transport` names another; discoverable
/// passkeys with user verification, as a password manager makes them.
pub fn options(transport: Option<&str>, user_verification: bool, resident_key: bool) -> Result<Value> {
    let transport = match transport.unwrap_or("internal").to_ascii_lowercase().as_str() {
        "internal" | "platform" => "internal",
        "usb" => "usb",
        "nfc" => "nfc",
        "ble" | "bluetooth" => "ble",
        "hybrid" | "cable" | "phone" => "cable",
        other => bail!("passkey on --authenticator takes internal, usb, nfc, ble or hybrid; got {other:?}"),
    };
    Ok(json!({
        "protocol": "ctap2",
        "ctap2Version": "ctap2_1",
        "transport": transport,
        "hasResidentKey": resident_key,
        "hasUserVerification": user_verification,
        "isUserVerified": user_verification,
        "automaticPresenceSimulation": true,
        "defaultBackupEligibility": transport == "internal",
        "defaultBackupState": transport == "internal",
    }))
}

/// Bytes from base64 in either alphabet, padded or not.
fn unbase64(s: &str) -> Result<Vec<u8>> {
    let url: String = s
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(url.as_bytes())
        .map_err(|e| anyhow::anyhow!("not base64 ({e}): {}", s.chars().take(24).collect::<String>()))
}

fn url64(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn std64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// The P-256 public key a PKCS#8 private key carries (the point after its
/// `[1]` tag), as SPKI DER; `None` for a key without it.
pub fn public_key(pkcs8: &[u8]) -> Option<Vec<u8>> {
    const POINT: [u8; 4] = [0x03, 0x42, 0x00, 0x04];
    const SPKI_P256: [u8; 26] = [
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48,
        0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    let at = pkcs8.windows(POINT.len()).rposition(|w| w == POINT)?;
    let point = pkcs8.get(at + 3..at + 3 + 65)?;
    Some([&SPKI_P256[..], point].concat())
}

/// Chrome's `WebAuthn.Credential` as Playwright's credential object.
pub fn to_portable(c: &Value) -> Result<Value> {
    let bytes = |k: &str| c[k].as_str().map(unbase64).transpose();
    let private = bytes("privateKey")?.unwrap_or_default();
    let mut out = json!({
        "id": url64(&bytes("credentialId")?.unwrap_or_default()),
        "rpId": c["rpId"],
        "userHandle": bytes("userHandle")?.map(|b| url64(&b)),
        "privateKey": url64(&private),
        "publicKey": public_key(&private).map(|b| url64(&b)),
        "signCount": c["signCount"].as_u64().unwrap_or(0),
    });
    for (from, to) in [("userName", "userName"), ("userDisplayName", "userDisplayName")] {
        if let Some(v) = c[from].as_str().filter(|v| !v.is_empty()) {
            out[to] = json!(v);
        }
    }
    if c["isResidentCredential"] == false {
        out["discoverable"] = json!(false);
    }
    Ok(out)
}

/// A credential from a file (Playwright's shape, or Chrome's own) as
/// `WebAuthn.addCredential` takes it.
pub fn from_portable(c: &Value) -> Result<Value> {
    let get = |k: &str| c[k].as_str().filter(|v| !v.is_empty());
    let (Some(id), Some(rp), Some(key)) = (get("id").or(get("credentialId")), get("rpId"), get("privateKey")) else {
        bail!("a passkey needs id, rpId and privateKey (as `state save` or Playwright's credentials.get() write them)");
    };
    let mut out = json!({
        "credentialId": std64(&unbase64(id)?),
        "rpId": rp,
        "privateKey": std64(&unbase64(key)?),
        "signCount": c["signCount"].as_u64().unwrap_or(0),
        "isResidentCredential": c["discoverable"] != false && c["isResidentCredential"] != false,
    });
    if let Some(h) = get("userHandle") {
        out["userHandle"] = json!(std64(&unbase64(h)?));
    }
    for k in ["userName", "userDisplayName"] {
        if let Some(v) = get(k) {
            out[k] = json!(v);
        }
    }
    Ok(out)
}

/// The passkeys in a JSON file: one credential, an array of them, or a
/// storage state with `credentials`.
pub fn read_file(v: &Value) -> Result<Vec<Value>> {
    let list = match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) if o.contains_key("credentials") => o["credentials"].as_array().cloned().unwrap_or_default(),
        Value::Object(_) => vec![v.clone()],
        _ => bail!("give a passkey, an array of them, or a state file with `credentials`"),
    };
    list.iter().map(from_portable).collect()
}

/// What `passkey list` shows of a credential: never its private key.
pub fn summary(c: &Value) -> Value {
    let p = to_portable(c).unwrap_or_else(|_| json!({}));
    let mut out = json!({ "id": p["id"], "rp_id": p["rpId"], "sign_count": p["signCount"] });
    if let Some(u) = p["userName"].as_str() {
        out["user"] = json!(u);
    }
    if c["isResidentCredential"] == false {
        out["discoverable"] = json!(false);
    }
    out
}

/// Several tabs' credentials as one list: each passkey once, the copy that
/// signed most recently (the highest counter) kept.
pub fn merge(all: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for c in all {
        match out.iter_mut().find(|o| o["credentialId"] == c["credentialId"]) {
            Some(o) if c["signCount"].as_u64() > o["signCount"].as_u64() => *o = c,
            Some(_) => {}
            None => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A P-256 PKCS#8 key with its public point (`openssl pkcs8 -topk8`).
    const KEY: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgumAFso3WG5cr3KyoczOwOrFSSk0bO4ANAn7uSlcph52hRANCAASGuUUfOTuGEbDpNGxXDFoToq1VvbXxiAerUTVMxjLba_pKgTSsdQ7mkQQpzYdO8AYpewi7ScW1iZHdHgzrxCxm";

    #[test]
    fn options_name_chromes_transports() {
        assert_eq!(options(None, true, true).unwrap()["transport"], "internal");
        assert_eq!(options(Some("hybrid"), true, true).unwrap()["transport"], "cable");
        let o = options(Some("usb"), false, false).unwrap();
        assert_eq!((o["hasUserVerification"].as_bool(), o["hasResidentKey"].as_bool()), (Some(false), Some(false)));
        assert_eq!(o["defaultBackupEligibility"], false);
        assert!(options(Some("pigeon"), true, true).is_err());
    }

    #[test]
    fn a_credential_round_trips_through_the_portable_shape() {
        let chrome = json!({
            "credentialId": "q83v", "isResidentCredential": true, "rpId": "localhost",
            "privateKey": std64(&unbase64(KEY).unwrap()), "userHandle": "dXNlcg==", "signCount": 3,
            "userName": "ada@example.com"
        });
        let p = to_portable(&chrome).unwrap();
        assert_eq!(
            (p["id"].as_str(), p["userHandle"].as_str(), p["signCount"].as_u64()),
            (Some("q83v"), Some("dXNlcg"), Some(3))
        );
        assert_eq!(p["userName"], "ada@example.com");
        let spki = unbase64(p["publicKey"].as_str().unwrap()).unwrap();
        assert_eq!((spki.len(), spki[0], spki[26]), (91, 0x30, 0x04), "SPKI DER of an uncompressed P-256 point");
        let back = from_portable(&p).unwrap();
        for k in ["credentialId", "rpId", "privateKey", "userHandle", "signCount", "isResidentCredential"] {
            assert_eq!(back[k], chrome[k], "{k}");
        }
        assert!(summary(&chrome).get("private_key").is_none() && summary(&chrome)["user"] == "ada@example.com");
    }

    #[test]
    fn files_hold_one_passkey_a_list_or_a_state() {
        let one = json!({"id": "q83v", "rpId": "a.test", "privateKey": KEY});
        assert_eq!(read_file(&one).unwrap().len(), 1);
        assert_eq!(read_file(&json!([one, one])).unwrap().len(), 2);
        assert_eq!(read_file(&json!({"cookies": [], "credentials": [one]})).unwrap()[0]["rpId"], "a.test");
        assert!(read_file(&json!({"id": "q83v"})).unwrap_err().to_string().contains("needs id, rpId and privateKey"));
    }

    #[test]
    fn merging_keeps_the_copy_that_signed_last() {
        let a = json!({"credentialId": "A", "signCount": 1});
        let a2 = json!({"credentialId": "A", "signCount": 4});
        let b = json!({"credentialId": "B", "signCount": 0});
        let m = merge([a, b, a2]);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["signCount"], 4);
    }
}
