//! The parity check's native half: every committed request answered by
//! [`mako_wasm::call`] exactly as committed. `scripts/parity.mjs` holds the
//! WebAssembly build to the same answers.
//!
//! `BLESS_WASM_PARITY=1` rewrites the answers after a deliberate change.

use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity")
}

#[test]
fn the_module_answers_every_committed_request_as_committed() {
    let requests = std::fs::read_to_string(dir().join("requests.jsonl")).expect("requests");
    let answers: Vec<String> = requests
        .lines()
        .filter(|l| !l.is_empty())
        .map(mako_wasm::call)
        .collect();
    let path = dir().join("answers.jsonl");
    let got = answers.join("\n") + "\n";
    if std::env::var_os("BLESS_WASM_PARITY").is_some() {
        std::fs::write(&path, got).expect("write answers");
        return;
    }
    let want = std::fs::read_to_string(&path).expect("answers; bless with BLESS_WASM_PARITY=1");
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "request {} answers differently", i + 1);
    }
    assert_eq!(got.lines().count(), want.lines().count(), "answer count");
}

/// SR-099: input over the limit is refused before anything is parsed.
#[test]
fn an_oversized_message_is_refused_unparsed() {
    let text = "UNA:+.? '".to_owned() + &"X".repeat(mako_wasm::MAX_INPUT_BYTES + 1);
    let answer = mako_wasm::call(&serde_json::json!({ "op": "check", "text": text }).to_string());
    assert!(answer.contains("\"eingabe_zu_gross\""), "{answer}");
}
