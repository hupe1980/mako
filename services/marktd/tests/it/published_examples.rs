//! Guard: every `curl` write example the `marktd` guide and README print is a
//! body `marktd` accepts.
//!
//! An example is the first thing an integrator runs, and it is pasted as is.
//! Every request type here denies unknown fields and most require fields the
//! example has to supply, so a body that drifted from its type is a 422 on the
//! integrator's first call. This deserialises each printed body into the type
//! its route takes — it does not run the handler, so authorisation and the
//! business rules stay the integration suites' business.
//!
//! A printed write to a route this file does not map fails too: the table
//! below grows with the examples, never silently behind them.

use std::path::{Path, PathBuf};

use marktd::handlers::{
    device::{PutKonfigurationenRequest, UpsertZaehlerRequest},
    mmma_preise::{MmmStromUpsertRequest, MmmaGasUpsertRequest},
    partner::PartnerUpsertRequest,
    preisblatt::PreisblattMessungUpsertRequest,
    subscription::SubscriptionUpsertRequest,
};

/// The pages whose examples this checks.
const PAGES: &[&str] = &[
    "site/content/docs/services/marktd.md",
    "services/marktd/README.md",
];

/// The repository root, from this test's own location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above services/marktd")
}

/// Split a shell command line into words the way `sh` does for the cases the
/// pages print: `'…'` literal, `"…"` with `\"`, `\\`, `\$` escapes, and a
/// backslash-newline as whitespace. Stops at an unquoted newline.
fn shell_words(src: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\' | '$')) => {
                            cur.extend(chars.next());
                        }
                        _ => cur.push(q),
                    }
                }
            }
            '\\' if chars.peek() == Some(&'\n') => {
                chars.next();
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\n' => break,
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// One printed `curl` write: where it is, its method, path and body.
struct Write {
    at: String,
    method: String,
    path: String,
    body: Option<String>,
}

/// Every `curl -X PUT|POST|PATCH` in the `bash`/`sh` blocks of a page.
fn writes(name: &str, md: &str) -> Vec<Write> {
    let mut out = Vec::new();
    let mut in_shell = false;
    let mut offset = 0;
    for (n, line) in md.lines().enumerate() {
        let start = offset;
        offset += line.len() + 1;
        let t = line.trim_start();
        if t.starts_with("```") {
            in_shell = !in_shell && matches!(t.trim_start_matches('`').trim(), "bash" | "sh");
            continue;
        }
        if !in_shell || !t.starts_with("curl ") {
            continue;
        }
        let mut w = Write {
            at: format!("{name}:{}", n + 1),
            method: "GET".to_owned(),
            path: String::new(),
            body: None,
        };
        let mut it = shell_words(&md[start..]).into_iter().skip(1);
        while let Some(word) = it.next() {
            match word.as_str() {
                "-X" | "--request" => w.method = it.next().unwrap_or_default(),
                "-d" | "--data" | "--data-raw" => w.body = it.next(),
                "-H" | "--header" => {
                    it.next();
                }
                u if u.contains("://") => {
                    let after_host = u.splitn(4, '/').nth(3).unwrap_or_default();
                    w.path = format!("/{}", after_host.split('?').next().unwrap_or_default());
                }
                _ => {}
            }
        }
        if w.body.is_some() && w.method == "GET" {
            w.method = "POST".to_owned();
        }
        if w.method != "GET" && w.method != "DELETE" {
            out.push(w);
        }
    }
    out
}

/// A shell expansion the page leaves for the reader's shell — `${VAR}`,
/// `$VAR`, `$(uuidgen)` — replaced by a value of the shape it stands for.
fn expand(body: &str) -> String {
    const UUID: &str = "3f2b0c1e-7a4d-4e5f-9b8a-1c2d3e4f5a6b";
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let skip = if let Some(t) = tail.strip_prefix('{') {
            t.find('}').map_or(0, |j| j + 2)
        } else if let Some(t) = tail.strip_prefix('(') {
            t.find(')').map_or(0, |j| j + 2)
        } else {
            tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len())
        };
        out.push_str(UUID);
        rest = &tail[skip..];
    }
    out.push_str(rest);
    out
}

fn parses<T: serde::de::DeserializeOwned>(body: &str) -> Result<(), String> {
    serde_json::from_str::<T>(body)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// `PUT …/konfigurationsprodukte` takes a bare array the handler checks itself:
/// each element a BO4E `Konfigurationsprodukt` with a non-empty `produktcode`.
fn konfigurationsprodukte(body: &str) -> Result<(), String> {
    let items: Vec<rubo4e::current::Konfigurationsprodukt> =
        serde_json::from_str(body).map_err(|e| e.to_string())?;
    match items
        .iter()
        .position(|k| k.produktcode.as_deref().is_none_or(str::is_empty))
    {
        Some(i) => Err(format!("element [{i}] has no produktcode")),
        None => Ok(()),
    }
}

/// `PUT /api/v1/zaehler/{id}` — the envelope, then the BO4E gate the handler
/// runs on `data`.
fn zaehler(body: &str) -> Result<(), String> {
    let req: UpsertZaehlerRequest = serde_json::from_str(body).map_err(|e| e.to_string())?;
    mako_markt::bo4e::decode::<rubo4e::current::Zaehler>(req.data)
        .map(drop)
        .map_err(|e| e.to_json().to_string())
}

type Check = fn(&str) -> Result<(), String>;

/// Route pattern (`{}` = one path segment) → the body type its handler takes.
const ROUTES: &[(&str, &str, Check)] = &[
    ("PUT", "/api/v1/partners/{}", parses::<PartnerUpsertRequest>),
    (
        "PUT",
        "/api/v1/subscriptions/{}",
        parses::<SubscriptionUpsertRequest>,
    ),
    (
        "PUT",
        "/api/v1/preisblaetter-messung/{}",
        parses::<PreisblattMessungUpsertRequest>,
    ),
    (
        "PUT",
        "/api/v1/mmma-preise/gas/{}/{}",
        parses::<MmmaGasUpsertRequest>,
    ),
    (
        "PUT",
        "/api/v1/mmm-preise/strom/{}/{}",
        parses::<MmmStromUpsertRequest>,
    ),
    ("PUT", "/api/v1/zaehler/{}", zaehler),
    (
        "PUT",
        "/api/v1/zaehler/{}/geraete/{}/konfigurationen",
        parses::<PutKonfigurationenRequest>,
    ),
    (
        "PUT",
        "/api/v1/zaehler/{}/register",
        parses::<mako_markt::repository::ZaehlzeitRegisterRecord>,
    ),
    (
        "PUT",
        "/api/v1/zaehler-register/{}/saisons",
        parses::<mako_markt::repository::ZaehlzeitSaisonRecord>,
    ),
    (
        "PUT",
        "/api/v1/steuerbare-ressourcen/{}/konfigurationsprodukte",
        konfigurationsprodukte,
    ),
];

fn route_matches(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let a: Vec<&str> = path.trim_matches('/').split('/').collect();
    p.len() == a.len() && p.iter().zip(&a).all(|(p, a)| *p == "{}" || p == a)
}

/// What is wrong with one printed write, if anything.
fn write_defect(w: &Write) -> Option<String> {
    let body = w.body.as_deref()?;
    let Some((_, _, check)) = ROUTES
        .iter()
        .find(|(m, p, _)| *m == w.method && route_matches(p, &w.path))
    else {
        return Some(format!(
            "{} {} prints a body no route in ROUTES maps — add the route and its type",
            w.method, w.path
        ));
    };
    check(&expand(body))
        .err()
        .map(|e| format!("{} {}: marktd refuses the body — {e}", w.method, w.path))
}

#[test]
fn every_printed_write_example_is_a_body_marktd_accepts() {
    let root = repo_root();
    let mut found = 0;
    let mut wrong = Vec::new();
    for rel in PAGES {
        let md = std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("{rel} is not readable: {e}"));
        for w in writes(rel, &md) {
            found += 1;
            if let Some(d) = write_defect(&w) {
                wrong.push(format!("{}: {d}", w.at));
            }
        }
    }
    assert!(found > 0, "the extraction found no write example at all");
    assert!(
        wrong.is_empty(),
        "printed marktd examples that would be refused:\n{}",
        wrong.join("\n")
    );
}

/// The check has to see the defect it exists for: a body naming a field the
/// type does not have, and one missing a field the type requires.
#[test]
fn a_planted_subscription_example_with_the_wrong_field_names_is_refused() {
    let md = "```bash\ncurl -X PUT http://localhost:8180/api/v1/subscriptions/erp \\\n  \
              -H \"Content-Type: application/json\" \\\n  \
              -d '{\"endpoint_url\": \"https://erp.example.com/hook\", \"secret\": \"s\"}'\n```\n";
    let ws = writes("fixture.md", md);
    assert_eq!(ws.len(), 1);
    assert!(write_defect(&ws[0]).is_some());
}
