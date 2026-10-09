//! Transport layer — HTTP client factory and cryptography.
//!
//! | Module | Purpose | Feature |
//! |--------|---------|---------|
//! | [`http`] | `reqwest::Client` builder with mTLS and retry config | `client` |
//! | [`jws`] | JWS ECDSA-SHA256 sign/verify for **directory records** | always |
//! | [`content_security`] | TR-03116-3 DIGEST/SIGNATURE for **electricity API calls** | always |

pub mod http;

pub mod jws;

pub mod content_security;
