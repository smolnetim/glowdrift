//! HTTPS access to a bridge with trust-on-first-use certificate pinning.
//!
//! Hue bridges present certificates no public CA vouches for, so the leaf certificate's SHA-256
//! is the trust anchor. A [`Bridge`] built without a pin records the first certificate it sees
//! and requires every later connection to match it; one built with a pin requires an exact
//! match from the first connection. The handshake signatures are still verified with the crypto
//! provider, so a peer must hold the pinned certificate's key.

use std::error::Error as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::HueError;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const USER_AGENT: &str = concat!("Glowdrift/", env!("CARGO_PKG_VERSION"));

/// Lowercase hex SHA-256 of a DER certificate.
pub(crate) fn fingerprint(der: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(der)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[derive(Debug)]
struct PinVerifier {
    algorithms: WebPkiSupportedAlgorithms,
    pin: Mutex<Option<String>>,
    mismatch: AtomicBool,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = fingerprint(end_entity);
        let mut pin = self.pin.lock().unwrap_or_else(PoisonError::into_inner);
        match pin.as_deref() {
            Some(expected) if !expected.eq_ignore_ascii_case(&seen) => {
                self.mismatch.store(true, Ordering::SeqCst);
                Err(rustls::Error::InvalidCertificate(
                    CertificateError::ApplicationVerificationFailure,
                ))
            }
            Some(_) => Ok(ServerCertVerified::assertion()),
            None => {
                *pin = Some(seen);
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Describes an error with its causes, without the URL.
pub(crate) fn describe(err: reqwest::Error) -> String {
    let err = err.without_url();
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// A connection target. The HTTP client pools connections, so one `Bridge` serves many
/// requests; it must be created and used off any async runtime thread.
pub(crate) struct Bridge {
    http: Client,
    base: String,
    app_key: Option<String>,
    verifier: Arc<PinVerifier>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// Validates a user-supplied address (`host` or `host:port`) and returns the HTTPS base URL.
fn base_url(address: &str) -> Result<String, HueError> {
    let invalid = || HueError::InvalidAddress(address.to_owned());
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let url = reqwest::Url::parse(&format!("https://{trimmed}")).map_err(|_| invalid())?;
    let clean = url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && !trimmed.contains(['/', '?', '#', '@', '\\', ' ']);
    if !clean {
        return Err(invalid());
    }
    Ok(format!("https://{trimmed}"))
}

impl Bridge {
    /// Connects to `address`. With `pin`, only that certificate is accepted; without, the first
    /// certificate seen is trusted and then required (see [`Bridge::pin`]).
    pub fn new(
        address: &str,
        pin: Option<&str>,
        app_key: Option<&str>,
        timeout: Duration,
    ) -> Result<Bridge, HueError> {
        let base = base_url(address)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinVerifier {
            algorithms: provider.signature_verification_algorithms,
            pin: Mutex::new(pin.map(str::to_ascii_lowercase)),
            mismatch: AtomicBool::new(false),
        });
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| HueError::Network(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::clone(&verifier) as Arc<dyn ServerCertVerifier>)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let http = Client::builder()
            .use_preconfigured_tls(tls)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| HueError::Network(describe(e)))?;
        Ok(Bridge {
            http,
            base,
            app_key: app_key.map(str::to_owned),
            verifier,
        })
    }

    /// The certificate fingerprint pinned so far: the supplied pin, or the first one seen.
    pub fn pin(&self) -> Option<String> {
        self.verifier
            .pin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn map_send_error(&self, err: reqwest::Error) -> HueError {
        if self.verifier.mismatch.swap(false, Ordering::SeqCst) {
            HueError::CertificateMismatch
        } else {
            HueError::Network(describe(err))
        }
    }

    /// Sends a request and returns the status and body. Does not interpret the status.
    pub fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Vec<u8>), HueError> {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(key) = &self.app_key {
            request = request.header("hue-application-key", key);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(|e| self.map_send_error(e))?;
        let status = response.status();
        let bytes = response.bytes().map_err(|e| self.map_send_error(e))?;
        Ok((status, bytes.to_vec()))
    }

    /// GET of a public v1 endpoint that always answers 200 with JSON.
    pub fn get_v1<T: DeserializeOwned>(&self, path: &str) -> Result<T, HueError> {
        let (status, body) = self.send(reqwest::Method::GET, path, None)?;
        if !status.is_success() {
            return Err(HueError::Bridge(format!("HTTP {status}")));
        }
        serde_json::from_slice(&body).map_err(|e| HueError::Parse(e.to_string()))
    }

    /// POST of a v1 body; the caller interprets the reply.
    pub fn post_v1(&self, path: &str, body: &Value) -> Result<Vec<u8>, HueError> {
        let (status, bytes) = self.send(reqwest::Method::POST, path, Some(body))?;
        if !status.is_success() {
            return Err(HueError::Bridge(format!("HTTP {status}")));
        }
        Ok(bytes)
    }

    /// GET of a CLIP v2 resource collection; returns its `data` array.
    pub fn get_v2<T: DeserializeOwned>(&self, resource: &str) -> Result<Vec<T>, HueError> {
        let (status, body) = self.send(
            reqwest::Method::GET,
            &format!("/clip/v2/resource/{resource}"),
            None,
        )?;
        super::api::parse_v2(status, &body)
    }

    /// PUT of a CLIP v2 resource.
    pub fn put_v2(&self, resource: &str, id: &str, body: &Value) -> Result<(), HueError> {
        let (status, reply) = self.send(
            reqwest::Method::PUT,
            &format!("/clip/v2/resource/{resource}/{id}"),
            Some(body),
        )?;
        super::api::parse_v2::<Value>(status, &reply).map(|_| ())
    }
}
