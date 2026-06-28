//! HTTP client factory with mTLS and retry support.
//!
//! All EDI-Energy API-Webdienste **must** use mutual TLS with an EMT.API
//! certificate issued by SM-PKI.  This module provides a typed configuration
//! struct and a builder function that produce a properly configured
//! [`reqwest::Client`].
//!
//! For local / integration testing, [`TlsConfig::insecure`] creates a client
//! that skips certificate verification — **never use in production**.

#[cfg(feature = "client")]
use reqwest::Client;

#[cfg(feature = "client")]
use crate::error::Error;

// ── TLS configuration ─────────────────────────────────────────────────────────

/// TLS configuration for the `reqwest::Client`.
#[derive(Debug, Default, Clone)]
pub struct TlsConfig {
    /// PEM-encoded PKCS#8 private key for the client (mTLS identity).
    pub client_key_pem: Option<String>,
    /// PEM-encoded X.509 certificate chain for the client (mTLS identity).
    pub client_cert_pem: Option<String>,
    /// Additional PEM-encoded root CA certificates to trust (e.g. SM-PKI CA).
    pub root_ca_pems: Vec<String>,
    /// When `true`, accept any server certificate regardless of validity.
    ///
    /// **Testing only.** Setting this in production is a security vulnerability.
    pub accept_invalid_certs: bool,
}

impl TlsConfig {
    /// Create a configuration with no certificates — for local / mock testing.
    pub fn insecure() -> Self {
        Self {
            accept_invalid_certs: true,
            ..Default::default()
        }
    }

    /// Create a configuration from PEM strings.
    ///
    /// `root_ca_pems` must contain the SM-PKI sub-CA certificates used to
    /// validate server certificates in the energy market.
    pub fn from_pem(
        client_cert_pem: impl Into<String>,
        client_key_pem: impl Into<String>,
        root_ca_pems: Vec<String>,
    ) -> Self {
        Self {
            client_cert_pem: Some(client_cert_pem.into()),
            client_key_pem: Some(client_key_pem.into()),
            root_ca_pems,
            accept_invalid_certs: false,
        }
    }
}

// ── Retry configuration ───────────────────────────────────────────────────────

/// Retry policy for idempotent HTTP requests.
///
/// The spec mandates that all electricity API services are idempotent
/// (`initialTransactionId` is the idempotency key).  Retries should use
/// exponential back-off with jitter.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (excluding the initial attempt).
    pub max_retries: u32,
    /// Base delay between retries in milliseconds.
    pub base_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 500,
        }
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Build a [`reqwest::Client`] from the given [`TlsConfig`].
///
/// The resulting client uses `rustls` and enables HTTP/1.1 + HTTP/2.
///
/// # Errors
///
/// Returns [`Error::Transport`] if:
/// - The mTLS identity PEM bytes are malformed.
/// - A root CA PEM is invalid.
/// - The `reqwest` builder fails to initialise.
#[cfg(feature = "client")]
pub fn build_client(config: &TlsConfig) -> Result<Client, Error> {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(config.accept_invalid_certs)
        // Respect the 10 s service-response timeout mandated by the spec.
        .timeout(std::time::Duration::from_secs(10));

    // Add custom root CAs (SM-PKI trust anchors).
    for pem in &config.root_ca_pems {
        let cert = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|e| Error::Transport(format!("root CA PEM: {e}")))?;
        builder = builder.add_root_certificate(cert);
    }

    // Attach mTLS client identity.
    if let (Some(cert_pem), Some(key_pem)) = (&config.client_cert_pem, &config.client_key_pem) {
        // reqwest::Identity::from_pem accepts a combined PEM buffer
        // (certificate chain followed by private key, or vice versa).
        let combined = format!("{cert_pem}\n{key_pem}");
        let identity = reqwest::Identity::from_pem(combined.as_bytes())
            .map_err(|e| Error::Transport(format!("mTLS identity PEM: {e}")))?;
        builder = builder.identity(identity);
    }

    builder.build().map_err(|e| Error::Transport(e.to_string()))
}
