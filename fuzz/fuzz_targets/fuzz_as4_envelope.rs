//! Fuzz target: `fuzz_as4_envelope`
//!
//! Exercises the AS4 SOAP envelope ingest path with arbitrary byte sequences.
//!
//! ## What is fuzzed
//!
//! The full receive pipeline for an inbound AS4 push message, including:
//! - MIME multipart boundary splitting
//! - SOAP 1.2 envelope XML parsing (via `roxmltree`)
//! - ebMS3 UserMessage / Receipt header parsing
//! - WS-Security header traversal
//! - Payload / attachment extraction
//!
//! Signature verification and PKIX chain validation are not exercised because
//! no signing certificate is configured in the session. The pipeline returns
//! a signature error on valid SOAP input; it must never panic.
//!
//! ## Threat model
//!
//! The AS4 ingest endpoint is the first trust boundary for bytes arriving from
//! external trading partners over the public BDEW AS4 endpoint. A panic or
//! OOM in the parsing layer before WS-Security authentication is completed
//! constitutes a pre-auth denial of service.
//!
//! ## Running locally
//!
//! Requires nightly + `cargo-fuzz`:
//!
//! ```text
//! cargo +nightly fuzz run fuzz_as4_envelope -- -max_total_time=300
//! ```
//!
//! Seed the corpus from real AS4 fixtures:
//!
//! ```text
//! cp demo/fixtures/utilmd-55001.edi fuzz/corpus/fuzz_as4_envelope/
//! ```

#![no_main]

use asx_rs::{
    core::SessionContext,
    as4::{
        As4ReceivePushRequest, As4ReceivePushSyncRequest, As4PushPolicy,
        receive_push_with_dedup_sync,
    },
    observability::EventBus,
    reliability::InMemoryDedupBackend,
};
use libfuzzer_sys::fuzz_target;
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    // Construct a minimal session context without X.509 material.
    // Signature verification will fail (expected) but must not panic.
    let session = match SessionContext::new("fuzz-session", "9900000000001", "strict") {
        Ok(s) => s,
        Err(_) => return, // construction failure → not a bug
    };

    let bus = match EventBus::new(4) {
        Ok(b) => b,
        Err(_) => return,
    };

    let dedup = InMemoryDedupBackend::default();

    let request = As4ReceivePushRequest {
        // Try both common Content-Type variants so the MIME splitter exercises
        // both the plain-XML path and the multipart/related path.
        http_content_type: "application/soap+xml; charset=UTF-8".to_owned(),
        payload: Arc::from(data),
        receipt_payload: None,
        policy: As4PushPolicy::default(),
        authenticated_sender_scope: Some(Arc::from("fuzz-sender")),
    };

    // The return value is intentionally ignored: the target only checks that
    // no panic (including stack overflow, assertion failure, or unwrap on None)
    // occurs for any byte sequence.
    let _ = receive_push_with_dedup_sync(
        &session,
        &bus,
        As4ReceivePushSyncRequest { request, dedup_backend: &dedup },
    );

    // Second pass with the multipart/related Content-Type to exercise the
    // MIME boundary path.
    let request2 = As4ReceivePushRequest {
        http_content_type:
            "multipart/related; type=\"application/xop+xml\"; boundary=\"fuzz\"; \
             start=\"<rootpart@fuzz>\"; start-info=\"application/soap+xml\""
                .to_owned(),
        payload: Arc::from(data),
        receipt_payload: None,
        policy: As4PushPolicy::default(),
        authenticated_sender_scope: Some(Arc::from("fuzz-sender")),
    };
    let _ = receive_push_with_dedup_sync(
        &session,
        &bus,
        As4ReceivePushSyncRequest { request: request2, dedup_backend: &dedup },
    );
});
