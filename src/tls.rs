//! TLS for the connections navigera opens itself: a `wss://` DevTools
//! endpoint (a browser in the cloud). rustls with ring, so there is no
//! OpenSSL to find and no C toolchain beyond ring's own; certificates are
//! checked by the operating system's verifier on macOS and Windows, and
//! against the system bundle (or `SSL_CERT_FILE`) on Linux, as the `http`
//! feature's requests are.

use std::sync::{Arc, OnceLock};

use anyhow::Result;

/// The client config every `wss://` connection uses, built once.
pub fn client_config() -> Result<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<std::result::Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt;
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .and_then(|b| b.with_platform_verifier())
                .map(|b| Arc::new(b.with_no_client_auth()))
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| anyhow::anyhow!("TLS setup (the system's certificates): {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_config_builds_on_this_machine() {
        let config = super::client_config().unwrap();
        assert!(config.alpn_protocols.is_empty(), "a WebSocket upgrade is HTTP/1.1, which needs no ALPN");
        assert!(std::sync::Arc::ptr_eq(&config, &super::client_config().unwrap()), "built once");
    }
}
