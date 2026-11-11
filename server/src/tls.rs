//! TLS helpers for the outbound agent (and tests of the same stack).
//!
//! `--tls-ca FILE` adds PEM CAs to the client root store (together with the
//! webpki roots). `--tls-fingerprint` pins the SHA-256 of the *leaf* (end
//! entity) certificate; a mismatch fails the handshake. When neither is set
//! the agent uses ureq's default verification. `--insecure` is not accepted.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Parse a SHA-256 fingerprint: 64 hex digits, optional colons/spaces/dashes,
/// and an optional `sha256:` / `sha256/` prefix. Case-insensitive.
pub fn parse_fingerprint(input: &str) -> Result<[u8; 32], String> {
    let trimmed = input.trim();
    let rest = if let Some(r) = strip_prefix_ci(trimmed, "sha256:") {
        r
    } else if let Some(r) = strip_prefix_ci(trimmed, "sha256/") {
        r
    } else {
        trimmed
    };
    let hex: String = rest.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let skipped = rest
        .chars()
        .filter(|c| !c.is_ascii_hexdigit() && !c.is_whitespace() && *c != ':' && *c != '-')
        .count();
    if skipped > 0 {
        return Err("tls fingerprint must be SHA-256 hex (optional colons or spaces)".into());
    }
    if hex.len() != 64 {
        return Err(format!(
            "tls fingerprint must be 32-byte SHA-256 hex (got {} hex digits)",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "tls fingerprint is not valid hex".to_string())?;
    }
    Ok(out)
}

fn strip_prefix_ci<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    if input.len() < prefix.len() {
        return None;
    }
    if input.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

pub fn fingerprint_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn leaf_sha256(der: &[u8]) -> [u8; 32] {
    let d = Sha256::digest(der);
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

fn load_pem_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let bytes = fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let mut reader = Cursor::new(bytes);
    let certs: Vec<CertificateDer<'static>> = rustls_pki_types_certs(&mut reader)?;
    if certs.is_empty() {
        return Err(format!("no certificates in {}", path.display()));
    }
    Ok(certs)
}

/// rustls 0.23 re-exports pki_types; parse PEM certs without a direct
/// rustls-pemfile dependency by scanning BEGIN CERTIFICATE blocks.
fn rustls_pki_types_certs(
    reader: &mut Cursor<Vec<u8>>,
) -> Result<Vec<CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    let bytes = reader.get_ref();
    let mut certs = Vec::new();
    for item in CertificateDer::pem_slice_iter(bytes) {
        let cert = item.map_err(|err| format!("invalid PEM CA: {err}"))?;
        certs.push(cert);
    }
    Ok(certs)
}

fn root_store(ca_file: &str) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if !ca_file.is_empty() {
        let path = Path::new(ca_file);
        for cert in load_pem_certs(path)? {
            roots
                .add(cert)
                .map_err(|err| format!("cannot add CA from {ca_file}: {err}"))?;
        }
    }
    if roots.is_empty() {
        return Err("TLS root store is empty".into());
    }
    Ok(roots)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Build a rustls client config when the operator set `--tls-ca` and/or
/// `--tls-fingerprint`. `Ok(None)` means "use ureq's default verifier".
pub fn client_config(
    ca_file: &str,
    fingerprint: &str,
) -> Result<Option<Arc<ClientConfig>>, String> {
    if ca_file.is_empty() && fingerprint.is_empty() {
        return Ok(None);
    }
    let pin = if fingerprint.is_empty() {
        None
    } else {
        Some(parse_fingerprint(fingerprint)?)
    };
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("tls: {err}"))?;

    let config = if let Some(pin) = pin {
        let inner = if ca_file.is_empty() {
            // Fingerprint-only: allow a self-signed panel after the pin matches.
            None
        } else {
            let roots = root_store(ca_file)?;
            Some(
                rustls::client::WebPkiServerVerifier::builder_with_provider(
                    Arc::new(roots),
                    provider.clone(),
                )
                .build()
                .map_err(|err| format!("tls verifier: {err}"))?,
            )
        };
        let verifier = Arc::new(PinningVerifier {
            pin,
            inner,
            provider,
        });
        builder
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth()
    } else {
        let roots = root_store(ca_file)?;
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Some(Arc::new(config)))
}

#[derive(Debug)]
struct PinningVerifier {
    pin: [u8; 32],
    inner: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl PinningVerifier {
    fn check_pin(&self, end_entity: &CertificateDer<'_>) -> Result<(), TlsError> {
        let actual = leaf_sha256(end_entity.as_ref());
        if actual.ct_eq(&self.pin).unwrap_u8() != 1 {
            return Err(TlsError::General(format!(
                "certificate fingerprint mismatch (got {}, expected {})",
                fingerprint_hex(&actual),
                fingerprint_hex(&self.pin)
            )));
        }
        Ok(())
    }
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        self.check_pin(end_entity)?;
        if let Some(inner) = &self.inner {
            inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        if let Some(inner) = &self.inner {
            return inner.verify_tls12_signature(message, cert, dss);
        }
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        if let Some(inner) = &self.inner {
            return inner.verify_tls13_signature(message, cert, dss);
        }
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        if let Some(inner) = &self.inner {
            return inner.supported_verify_schemes();
        }
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Shared ureq agent builder used by the outbound agent.
pub fn build_ureq_agent(
    ca_file: &str,
    fingerprint: &str,
    url: &str,
) -> Result<ureq::Agent, String> {
    if url.starts_with("http://") && !is_loopback_url(url) {
        eprintln!(
            "warning: {url} is plain http; the credential and every job travel unencrypted. Use HTTPS."
        );
    }
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(40))
        .timeout_write(std::time::Duration::from_secs(15));
    if let Some(config) = client_config(ca_file, fingerprint)? {
        builder = builder.tls_config(config);
    }
    Ok(builder.build())
}

pub fn is_loopback_url(url: &str) -> bool {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', '?'])
        .next()
        .unwrap_or("");
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let host = host.trim_matches(['[', ']']);
    host == "localhost" || host.starts_with("127.") || host == "::1"
}

/// Upgrade is allowed by default only when the panel URL is HTTPS (verified
/// TLS) or loopback HTTP. Never download an agent binary over plain remote http.
pub fn upgrade_transport_ok(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("https://") || (url.starts_with("http://") && is_loopback_url(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::ServerName;

    fn self_signed(name: &str) -> (Vec<u8>, Vec<u8>, [u8; 32]) {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        let cert = params.self_signed(&key).unwrap();
        let der = cert.der().to_vec();
        let pin = leaf_sha256(&der);
        let pem = cert.pem().into_bytes();
        let key_pem = key.serialize_pem().into_bytes();
        (pem, key_pem, pin)
    }

    #[test]
    fn parses_hex_fingerprint_with_colons_and_prefix() {
        let raw = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let a = parse_fingerprint(raw).unwrap();
        let b = parse_fingerprint(
            &raw.chars()
                .enumerate()
                .flat_map(|(i, c)| {
                    if i > 0 && i % 2 == 0 {
                        vec![':', c]
                    } else {
                        vec![c]
                    }
                })
                .collect::<String>(),
        )
        .unwrap();
        let c = parse_fingerprint(&format!("SHA256:{raw}")).unwrap();
        let d = parse_fingerprint(&raw.to_ascii_uppercase()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
        assert!(parse_fingerprint("deadbeef").is_err());
        assert!(parse_fingerprint("xyz").is_err());
    }

    #[test]
    fn pinning_verifier_accepts_matching_leaf_and_rejects_mismatch() {
        let (pem, _, pin) = self_signed("panel.example");
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&pem)
            .map(|c| c.unwrap())
            .collect();
        let end = certs[0].clone();
        let verifier = PinningVerifier {
            pin,
            inner: None,
            provider: provider(),
        };
        let name = ServerName::try_from("panel.example").unwrap();
        let now = UnixTime::now();
        assert!(verifier
            .verify_server_cert(&end, &[], &name, &[], now)
            .is_ok());

        let mut wrong = pin;
        wrong[0] ^= 0xff;
        let bad = PinningVerifier {
            pin: wrong,
            inner: None,
            provider: provider(),
        };
        let err = bad
            .verify_server_cert(&end, &[], &name, &[], now)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("fingerprint mismatch"), "{msg}");
    }

    #[test]
    fn custom_ca_config_builds() {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-tls-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (pem, _, pin) = self_signed("localhost");
        let ca = dir.join("ca.pem");
        std::fs::write(&ca, &pem).unwrap();
        assert!(client_config(ca.to_str().unwrap(), "").unwrap().is_some());
        let hex = fingerprint_hex(&pin);
        assert!(client_config("", &hex).unwrap().is_some());
        assert!(client_config(ca.to_str().unwrap(), &hex).unwrap().is_some());
        assert!(client_config("", "").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_talks_to_self_signed_https_panel() {
        let dir = std::env::temp_dir().join(format!(
            "nginxwebui-tls-http-{}-{}",
            std::process::id(),
            crate::store::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params =
            rcgen::CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
                .unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "localhost");
        let cert = params.self_signed(&key).unwrap();
        let pem = cert.pem().into_bytes();
        let key_pem = key.serialize_pem().into_bytes();
        let pin = leaf_sha256(cert.der());
        let ca = dir.join("ca.pem");
        std::fs::write(&ca, &pem).unwrap();
        let key = key_pem;
        let server = tiny_http::Server::https(
            "127.0.0.1:0",
            tiny_http::SslConfig {
                certificate: pem,
                private_key: key,
            },
        )
        .expect("listen https");
        let port = server.server_addr().to_ip().expect("tcp").port();
        std::thread::spawn(move || {
            for _ in 0..8 {
                match server.recv() {
                    Ok(req) => {
                        let _ = req.respond(tiny_http::Response::from_string("ok"));
                    }
                    Err(_) => break,
                }
            }
        });
        let url = format!("https://127.0.0.1:{port}/healthz");
        let with_ca = build_ureq_agent(ca.to_str().unwrap(), "", &url).unwrap();
        let body = with_ca.get(&url).call().unwrap().into_string().unwrap();
        assert_eq!(body, "ok");

        let hex = fingerprint_hex(&pin);
        let with_pin = build_ureq_agent("", &hex, &url).unwrap();
        let body = with_pin.get(&url).call().unwrap().into_string().unwrap();
        assert_eq!(body, "ok");

        let mut wrong = pin;
        wrong[0] ^= 0xff;
        let bad = build_ureq_agent("", &fingerprint_hex(&wrong), &url).unwrap();
        assert!(
            bad.get(&url).call().is_err(),
            "mismatched fingerprint must fail"
        );

        // default roots do not trust a self-signed panel
        let plain = build_ureq_agent("", "", &url).unwrap();
        assert!(plain.get(&url).call().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
