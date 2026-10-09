//! Guard: every webhook-signature description an integrator can read matches
//! what [`mako_service::webhook::sign`] produces, and every printed verifier
//! accepts a real delivery.
//!
//! An integrator builds a receiver from the page, not from this crate. A page
//! describing another construction — one `sha256=<hex>` header over the body —
//! produces a verifier that refuses every delivery, or a Kafka head-end whose
//! records are all dropped as forged, and nothing on mako's side notices: the
//! sender is correct, the receiver is wrong, and the receiver is someone else's.
//!
//! Two checks, because a page can be wrong in prose and in code:
//!
//! 1. no page that names `webhook-signature` describes the `sha256=` shape;
//! 2. every Python block that reads `webhook-signature` is run, as printed,
//!    against a delivery this crate signed — it must accept it and refuse the
//!    same delivery with one byte of the body changed.
//!
//! `python3` is required: the gate already needs it for `makotest`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, from this test's own location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above crates/mako-service")
}

/// Every Markdown page an integrator reads: the site and every README.
fn published_pages() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    let root = repo_root();
    let mut pages = Vec::new();
    walk(&root.join("site/content"), &mut pages);
    pages.push(root.join("README.md"));
    for group in ["crates", "services"] {
        for e in std::fs::read_dir(root.join(group))
            .into_iter()
            .flatten()
            .flatten()
        {
            let readme = e.path().join("README.md");
            if readme.is_file() {
                pages.push(readme);
            }
        }
    }
    pages.sort();
    pages
}

/// The fenced blocks of `md` whose info string is `lang`, with their line.
fn fenced_blocks(md: &str, lang: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (n, line) in md.lines().enumerate() {
        let t = line.trim_start();
        match open.as_mut() {
            None if t.starts_with("```") && t.trim_start_matches('`').trim() == lang => {
                open = Some((n + 1, String::new()));
            }
            Some(_) if t.starts_with("```") => blocks.extend(open.take()),
            Some((_, body)) => {
                body.push_str(line);
                body.push('\n');
            }
            None => {}
        }
    }
    blocks
}

/// The pages naming `webhook-signature` that describe the `sha256=` shape.
fn pages_describing_a_hex_body_signature(pages: &[(String, String)]) -> Vec<String> {
    pages
        .iter()
        .filter(|(_, md)| md.contains("webhook-signature"))
        .flat_map(|(name, md)| {
            md.lines()
                .enumerate()
                .filter(|(_, l)| l.contains("sha256="))
                .map(move |(n, l)| format!("{name}:{}: {}", n + 1, l.trim()))
        })
        .collect()
}

fn read_pages() -> Vec<(String, String)> {
    let root = repo_root();
    published_pages()
        .into_iter()
        .map(|p| {
            let md = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{} is not readable: {e}", p.display()));
            let name = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
            (name, md)
        })
        .collect()
}

#[test]
fn no_page_describes_a_hex_signature_over_the_body() {
    let found = pages_describing_a_hex_body_signature(&read_pages());
    assert!(
        found.is_empty(),
        "these pages describe `webhook-signature` as `sha256=<hex>` over the body; \
         mako signs Standard Webhooks — `v1,<base64>` over `{{id}}.{{timestamp}}.{{body}}` \
         (`mako_service::webhook::sign`):\n{}",
        found.join("\n")
    );
}

/// The check has to see the shape it exists to refuse.
#[test]
fn a_planted_hex_signature_paragraph_is_found() {
    let planted = vec![(
        "fixture.md".to_owned(),
        "`webhook-signature` carries `sha256=<hex>` over the body.".to_owned(),
    )];
    assert_eq!(pages_describing_a_hex_body_signature(&planted).len(), 1);
}

/// Run one printed verifier against one delivery; `true` when it accepts.
///
/// The verifier is called by keyword with whichever of `body`, `secret` and
/// `headers` it declares — the printed functions agree on those names and
/// differ in order. A verifier accepts by returning something truthy (`True`,
/// or the id to deduplicate on) and refuses by returning falsy or raising.
fn python_accepts(verifier: &str, secret: &str, headers: &[(&str, String)], body: &[u8]) -> bool {
    const HARNESS: &str = r#"
import base64, inspect, json, sys
src = sys.stdin.read()
ns = {}
exec(compile(src, "<verifier>", "exec"), ns)
fns = [f for f in ns.values()
       if inspect.isfunction(f) and f.__code__.co_filename == "<verifier>"]
assert len(fns) == 1, "a verifier block defines exactly one function"
args = json.loads(sys.argv[1])
given = {"body": base64.b64decode(args["body"]),
         "secret": args["secret"],
         "headers": args["headers"]}
params = inspect.signature(fns[0]).parameters
missing = [p for p in params if p not in given]
assert not missing, "the verifier takes parameters the harness cannot supply: %r" % missing
try:
    ok = bool(fns[0](**{p: given[p] for p in params}))
except Exception:
    ok = False
print("ACCEPT" if ok else "REFUSE")
"#;
    use base64::Engine as _;
    let args = serde_json::json!({
        "secret": secret,
        "body": base64::engine::general_purpose::STANDARD.encode(body),
        "headers": headers.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
    });
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(HARNESS)
        .arg(args.to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3 is required to run the printed webhook verifiers");
    {
        use std::io::Write as _;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(verifier.as_bytes())
            .expect("the verifier is written to python3");
    }
    let out = child.wait_with_output().expect("python3 runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the harness failed on this verifier:\n{verifier}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout.trim() == "ACCEPT"
}

/// The Python verifier blocks the pages print: those defining a function over
/// an HMAC.
fn printed_verifiers(pages: &[(String, String)]) -> Vec<(String, String)> {
    pages
        .iter()
        .flat_map(|(name, md)| {
            fenced_blocks(md, "python")
                .into_iter()
                .filter(|(_, b)| b.contains("hmac") && b.contains("def "))
                .map(move |(line, b)| (format!("{name}:{line}"), b))
        })
        .collect()
}

#[test]
fn every_printed_verifier_accepts_a_real_delivery_and_refuses_a_tampered_one() {
    let verifiers = printed_verifiers(&read_pages());
    assert!(
        verifiers
            .iter()
            .any(|(at, _)| at.starts_with("site/content/docs/services/marktd.md")),
        "the marktd guide prints a verifier; the extraction must find it"
    );

    let secret = "whsec-test-secret";
    let id = "de.markt.malo.updated/3f2b0c1e-7a4d-4e5f-9b8a-1c2d3e4f5a6b";
    let ts = time::OffsetDateTime::now_utc().unix_timestamp();
    let body = br#"{"specversion":"1.0","type":"de.markt.malo.updated"}"#;
    let headers: Vec<(&str, String)> =
        mako_service::webhook::headers(secret.as_bytes(), id, ts, body).into();

    let mut tampered = body.to_vec();
    tampered[2] ^= 0x01;

    let mut wrong = Vec::new();
    for (at, verifier) in &verifiers {
        if !python_accepts(verifier, secret, &headers, body) {
            wrong.push(format!("{at}: refuses a delivery `webhook::sign` produced"));
        }
        if python_accepts(verifier, secret, &headers, &tampered) {
            wrong.push(format!("{at}: accepts a delivery whose body was changed"));
        }
    }
    assert!(
        wrong.is_empty(),
        "printed verifiers that do not hold:\n{}",
        wrong.join("\n")
    );
}

/// The harness has to be able to tell a wrong verifier from a right one.
#[test]
fn a_planted_hex_verifier_is_refused_by_the_harness() {
    let planted = r#"
import hmac, hashlib

def verify(body: bytes, secret: str, headers) -> bool:
    received = headers["webhook-signature"].removeprefix("sha256=")
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, received)
"#;
    let secret = "s";
    let ts = time::OffsetDateTime::now_utc().unix_timestamp();
    let headers: Vec<(&str, String)> =
        mako_service::webhook::headers(secret.as_bytes(), "id", ts, b"{}").into();
    assert!(!python_accepts(planted, secret, &headers, b"{}"));
}
