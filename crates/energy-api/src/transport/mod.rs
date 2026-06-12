//! Transport layer — HTTP client factory and cryptography.
//!
//! | Module | Purpose | Feature |
//! |--------|---------|---------|
//! | [`http`] | `reqwest::Client` builder with mTLS and retry config | `client` |
//! | [`jws`] | JWS ECDSA-SHA256 sign/verify for **directory records** | `crypto` |
//! | [`content_security`] | TR-03116-3 DIGEST/SIGNATURE for **electricity API calls** | `crypto` |

pub mod http;

#[cfg(feature = "crypto")]
pub mod jws;

#[cfg(feature = "crypto")]
pub mod content_security;
