//! Fuzz target: `fuzz_as4_inbound`
//!
//! Feeds arbitrary requests to the `asx-rs` AS4 inbox (`asx::Inbound` built on
//! `Profile::bdew()`), the service `makod` mounts at `/as4/inbox/<MP-ID>`.
//!
//! ## What is fuzzed
//!
//! Everything an inbox does with a request before and while authenticating it:
//! the size limit, MIME multipart splitting, SOAP 1.2 and ebMS3 header parsing,
//! the WS-Security header with its `X509PKIPathv1` token, signature reference
//! resolution and digest checks, and the refusal signals. A random body carries
//! no valid signature from the test SM-PKI, so the handler is never reached;
//! the inbox must answer every input and never panic.
//!
//! ## Threat model
//!
//! The AS4 inbox is the first trust boundary for bytes from the public BDEW AS4
//! endpoint. A panic or unbounded allocation before WS-Security authentication
//! completes is a pre-authentication denial of service.
//!
//! ## Running locally
//!
//! Requires nightly + `cargo-fuzz`:
//!
//! ```text
//! cargo +nightly fuzz run fuzz_as4_inbound -- -max_total_time=300
//! ```

#![no_main]
#![deny(unsafe_code)]

use std::sync::OnceLock;

use libfuzzer_sys::{arbitrary, fuzz_target};
use tower_service::Service as _;

/// One request: its `Content-Type` and its body.
#[derive(arbitrary::Arbitrary, Debug)]
struct Input {
    content_type: String,
    body: Vec<u8>,
}

/// The inbox under test and the runtime it runs on, built once.
struct Harness {
    runtime: tokio::runtime::Runtime,
    inbound: asx::Inbound,
}

fn harness() -> &'static Harness {
    static HARNESS: OnceLock<Harness> = OnceLock::new();
    HARNESS.get_or_init(|| {
        let ca = asx::testkit::pki::TestCa::sm_pki();
        let party = ca.bdew_party("9900001000001");
        let client = asx::as4::As4::builder(asx::Profile::bdew().max_message_bytes(64 << 20))
            .local(party.local())
            .trust_anchor(ca.anchor())
            .http_client(asx::testkit::loopback::Loopback::new())
            .build()
            .expect("the fuzzed inbox builds");
        let inbound = client.inbound(
            asx::MemoryDedup::default(),
            |_msg: asx::Verified<asx::as4::UserMessage>| async { Ok(()) },
        );
        Harness {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime"),
            inbound,
        }
    })
}

fuzz_target!(|input: Input| {
    let h = harness();
    let Ok(request) = http::Request::builder()
        .method("POST")
        .uri("/as4/inbox/9900001000001")
        .header(http::header::CONTENT_TYPE, input.content_type.as_str())
        .body(http_body_util::Full::new(bytes::Bytes::from(input.body)))
    else {
        // A Content-Type that is no header value never reaches a server.
        return;
    };
    let mut inbound = h.inbound.clone();
    let response = h.runtime.block_on(inbound.call(request));
    let Ok(response) = response;
    // Every input is answered: a refusal, a 413, never a success for bytes
    // that carry no valid signature.
    assert!(!response.status().is_success());
});
