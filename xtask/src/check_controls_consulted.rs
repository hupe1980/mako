//! Guard: every declared control is consulted by something.
//!
//! Every other check in `just ci` asks whether a thing is *there* — a column, a
//! grant, a constant. A control that is stored, returned, documented and never
//! compared passes all of them, and it is the dominant Critical shape in this
//! codebase: it looks configured, so nobody checks that anything reads it. This
//! guard asks the other question, for five kinds of control.
//!
//! | Kind | Discovered from | Consulted when |
//! |---|---|---|
//! | Frist constant | `pub const` named `*_FRIST*`, `*_WERKTAGE`, `*_MONATE` in `crates/*/src` | its name appears in production code beyond its definition |
//! | Meldepflicht row | each `Meldepflicht` of `mako_fristen::meldung`, named `pid←triggers` | production code outside `mako-fristen` calls `meldepflichten_for(<trigger>)` with one of its triggers as a literal, or `meldung::all(`; a step owed on silence (`AusbleibendeAntwort`, named by its PID) when its constant is named there |
//! | Credential field | `pub` `String`/`SecretString` fields named for a secret, key or token in `services/*/src/config.rs` and `mako-service` | `.field` appears in the owning crate's production code |
//! | Role column | a `rolle`/`role`/`roles`/`marktrolle`/`marktrole`/`*_rolle`/`*_role` column in `services/*/migrations` | the declared site in [`ROLE_COLUMNS`] still contains its needle |
//! | Feature-gated security module | `#[cfg(feature …)]` on a module named for auth, Cedar, OIDC, signing, TLS or crypto | the declared `Cargo.toml` in [`FEATURE_GATES`] still enables the feature |
//!
//! „Production code" is `crates/*/src` and `services/*/src` up to the first
//! `#[cfg(test)]` of a file: a control read only by its own unit test is the
//! defect, not the refutation of it.
//!
//! ## The lists
//!
//! A discovered candidate that no list names fails the guard, so a list cannot
//! silently be narrower than the tree. An unread control is admitted only with
//! a reason and the milestone that wires or removes it, and for the three kinds
//! whose consultation is computed the exemption list only shrinks: an entry
//! whose control gains a reader fails until the entry goes. A reason is not
//! checked against the code — re-read the reasons when the list is touched.
//!
//! A role column's and a feature gate's consultation is declared rather than
//! computed: a `rolle = EXCLUDED.rolle` in an upsert looks like a comparison
//! and is not one, so the entry names the file and the text that reads it, and
//! the guard fails when that text is gone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;

/// An unread control, admitted with what removes it.
struct Unread {
    /// The control's name.
    name: &'static str,
    /// Why nothing reads it yet.
    reason: &'static str,
    /// The milestone that wires or removes it.
    milestone: &'static str,
}

/// Frist constants nothing reads. May only shrink.
const FRIST_UNREAD: &[Unread] = &[
    Unread {
        name: "BESTELLUNG_FRIST_WT",
        reason: "the MSBN's Bestellung window against a standing Angebot (WiM Teil 1 Kap. 3.2.2 Nr. 3); the Geräteübernahme workflow arms only the Angebot and Bestätigung windows",
        milestone: "M11",
    },
    Unread {
        name: "KONZESSION_BEKANNTGABE_MONATE",
        reason: "published without a date derived from it: the AWH leaves open what the 18 months count from",
        milestone: "M12",
    },
    Unread {
        name: "MODELLWECHSEL_VORLAUF_MONATE",
        reason: "the Modellwechsel An-/Abmeldung lead time; no Modell-2 starter applies it yet",
        milestone: "M13",
    },
    Unread {
        name: "STAMMDATEN_BESTELLUNG_WERKTAGE",
        reason: "the Bearbeitungsstand to a Bestellung einer Stammdatenänderung (GPKE Teil 4 Kap. 1.5.2); no catalogue row or workflow applies it",
        milestone: "M11",
    },
    Unread {
        name: "STAMMDATEN_GUELTIG_AB_MIN_WERKTAGE",
        reason: "Redispatch Stammdaten lead time; the module is not wired",
        milestone: "M33",
    },
];

/// Meldepflicht rows (by PID) nothing reads. May only shrink.
const MELDUNG_UNREAD: &[Unread] = &[
    Unread {
        name: "44036←44001",
        reason: "GeLi Gas 3.0 Kap. 3.2.3 (S. 9) owes it beside the Abmeldungsanfrage 44010, and no GNB door sends a 44010 yet, so nothing declares it owed",
        milestone: "M11",
    },
    Unread {
        name: "44037←44001",
        reason: "GeLi Gas 3.0 Kap. 3.2.3 (S. 9) owes it beside the Abmeldungsanfrage 44010, and no GNB door sends a 44010 yet, so nothing declares it owed",
        milestone: "M11",
    },
    Unread {
        name: "44038←44001",
        reason: "whether a LFZ is displaced is decided by no source processd reads (nb_module.rs: the 55038/44038 is not derived from the Anmeldung decision), so no process can know it is owed",
        milestone: "M11",
    },
    Unread {
        name: "55038←55001/55077",
        reason: "whether a LFZ is displaced is decided by no source processd reads (nb_module.rs: the 55038 is not derived from the Anmeldung decision); the Abmeldeanfrage door names it `nicht_entschieden`",
        milestone: "M11",
    },
    Unread {
        name: "55037←55607",
        reason: "no NB-side workflow sends the Ankündigung Zuordnung 55607 it is triggered by",
        milestone: "M11",
    },
];

/// Credential fields nothing reads, as `service/field`. May only shrink.
const CREDENTIAL_UNREAD: &[Unread] = &[];

/// Where a declared control is consulted: a file and the text in it that
/// reads the control. A control nothing reads is removed, not declared.
enum Site {
    At(&'static str, &'static str),
}

/// Role columns, as `(service, column, site)`.
const ROLE_COLUMNS: &[(&str, &str, Site)] = &[
    (
        "edmd",
        "auftraggeber_rolle",
        Site::At(
            "services/edmd/src/mcp_server.rs",
            "AND auftraggeber_rolle = 'NB'",
        ),
    ),
    (
        "marktd",
        "marktrole",
        Site::At("services/marktd/src/fanout.rs", "row.marktrole.as_deref()"),
    ),
    (
        "marktd",
        "marktrolle",
        Site::At(
            "services/marktd/src/handlers/partner.rs",
            "record.marktrolle == Some(",
        ),
    ),
    (
        "marktd",
        "roles",
        Site::At("services/marktd/src/pg/subscription.rs", "= ANY(roles)"),
    ),
    (
        "marktd",
        "msb_rolle",
        Site::At(
            "services/makod/src/orchestrator/commands_api/eog_stammdaten.rs",
            "(Some(MsbRolle::Grundzustaendig), _) => Some(\"Z39\")",
        ),
    ),
    (
        "processd",
        "mdm_role",
        Site::At("services/processd/src/obs/pg.rs", "mdm_role = $4"),
    ),
    (
        "processd",
        "marktrolle",
        Site::At(
            "services/processd/src/server.rs",
            "marktrolle: entry.marktrolle.clone()",
        ),
    ),
];

/// Feature-gated security modules, as `(crate dir, module, site)` — the site is
/// a `Cargo.toml` that enables the feature.
const FEATURE_GATES: &[(&str, &str, Site)] = &[
    (
        "crates/mako-service",
        "cedar",
        Site::At("services/makod/Cargo.toml", "\"oidc\", \"cedar\""),
    ),
    (
        "crates/mako-service",
        "cedar_schema",
        Site::At("services/makod/Cargo.toml", "\"oidc\", \"cedar\""),
    ),
    (
        "crates/mako-service",
        "mcp_auth",
        Site::At("services/makod/Cargo.toml", "\"oidc\", \"cedar\""),
    ),
    (
        "crates/mako-service",
        "oidc",
        Site::At("services/makod/Cargo.toml", "\"oidc\", \"cedar\""),
    ),
];

/// One finding.
type Finding = String;

/// Run the guard. Returns `true` when every control is consulted or admitted.
pub fn run(root: &Path) -> bool {
    let crates = dirs(&root.join("crates"));
    let services = dirs(&root.join("services"));
    if crates.is_empty() || services.is_empty() {
        eprintln!("check-controls-consulted: no crate or service found — the layout moved");
        return false;
    }
    let corpus = Corpus::load(root, &crates, &services);
    let mut findings = Vec::new();
    findings.extend(frist_constants(&corpus));
    findings.extend(meldung_rows(root, &corpus));
    findings.extend(credential_fields(root, &corpus, &services));
    findings.extend(role_columns(root, &services));
    findings.extend(feature_gates(root, &corpus));
    if findings.is_empty() {
        println!(
            "check-controls-consulted: every declared control in {} file(s) names where it is \
             consulted, or why nothing does",
            corpus.files.len()
        );
        return true;
    }
    eprintln!("check-controls-consulted: {} finding(s):", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(p) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

/// Production sources, workspace-relative, each cut at its first `#[cfg(test)]`.
struct Corpus {
    files: Vec<(String, String)>,
}

impl Corpus {
    fn load(root: &Path, crates: &[PathBuf], services: &[PathBuf]) -> Self {
        let mut paths = Vec::new();
        for d in crates.iter().chain(services) {
            collect_rs(&d.join("src"), &mut paths);
        }
        paths.sort();
        let files = paths
            .into_iter()
            .filter_map(|p| {
                let src = std::fs::read_to_string(&p).ok()?;
                let rel = rel(root, &p);
                let cut = src.find("#[cfg(test)]").map_or(src.as_str(), |i| &src[..i]);
                Some((rel, cut.to_owned()))
            })
            .collect();
        Self { files }
    }

    /// Occurrences of `re` across the corpus, restricted to files whose path
    /// passes `keep`.
    fn count(&self, re: &Regex, keep: impl Fn(&str) -> bool) -> usize {
        self.files
            .iter()
            .filter(|(p, _)| keep(p))
            .map(|(_, s)| re.find_iter(s).count())
            .sum()
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The shrink-only rule for a computed kind: every unread candidate must be
/// listed, and every listed name must still be an unread candidate.
fn reconcile(
    kind: &str,
    unread: &BTreeSet<String>,
    all: &BTreeSet<String>,
    list: &[Unread],
) -> Vec<Finding> {
    let mut out = Vec::new();
    let listed: BTreeSet<&str> = list.iter().map(|u| u.name).collect();
    for name in unread {
        if !listed.contains(name.as_str()) {
            out.push(format!(
                "{kind} `{name}` is read by nothing — wire it, or list it with a reason and \
                 the milestone that removes it"
            ));
        }
    }
    for u in list {
        if u.reason.is_empty() || u.milestone.is_empty() {
            out.push(format!(
                "{kind} `{}` is listed without a reason or milestone",
                u.name
            ));
        }
        if !all.contains(u.name) {
            out.push(format!(
                "{kind} `{}` is listed but no longer exists — remove the entry",
                u.name
            ));
        } else if !unread.contains(u.name) {
            out.push(format!(
                "{kind} `{}` is listed as unread but something reads it now — remove the entry",
                u.name
            ));
        }
    }
    out
}

// ── Frist constants ──────────────────────────────────────────────────────────

fn frist_constants(corpus: &Corpus) -> Vec<Finding> {
    let def = Regex::new(
        r"pub const ([A-Z0-9_]*(?:_FRIST_|_FRIST\b|_WERKTAGE\b|_MONATE\b)[A-Z0-9_]*)\s*:",
    )
    .expect("pattern");
    let mut all = BTreeSet::new();
    for (path, src) in &corpus.files {
        if !path.starts_with("crates/") {
            continue;
        }
        for c in def.captures_iter(src) {
            all.insert(c[1].to_owned());
        }
    }
    let unread: BTreeSet<String> = all
        .iter()
        .filter(|name| {
            let re = Regex::new(&format!(r"\b{name}\b")).expect("ident");
            corpus.count(&re, |_| true) <= 1
        })
        .cloned()
        .collect();
    reconcile("Frist constant", &unread, &all, FRIST_UNREAD)
}

// ── Meldepflicht rows ────────────────────────────────────────────────────────

fn meldung_rows(root: &Path, corpus: &Corpus) -> Vec<Finding> {
    let Ok(src) = std::fs::read_to_string(root.join("crates/mako-fristen/src/meldung.rs")) else {
        return vec![
            "crates/mako-fristen/src/meldung.rs is unreadable — the catalogue moved".into(),
        ];
    };
    let outside = |p: &str| !p.starts_with("crates/mako-fristen/");
    let pid = |s: &str| s.trim().replace('_', "");
    // A PID recurs in several Sequenzdiagramme with a different window each,
    // so a row is its PID *and* the triggers that open it, and it is read by a
    // lookup naming one of those triggers.
    let meldepflicht =
        Regex::new(r"(?s)Meldepflicht\s*\{\s*pid:\s*([0-9_]+),.*?triggered_by:\s*&\[([^\]]*)\]")
            .expect("pattern");
    let rows: Vec<(String, Vec<String>)> = meldepflicht
        .captures_iter(&src)
        .map(|c| {
            let triggers: Vec<String> =
                c[2].split(',').map(pid).filter(|t| !t.is_empty()).collect();
            (format!("{}←{}", pid(&c[1]), triggers.join("/")), triggers)
        })
        .collect();
    let reads_all = corpus.count(&Regex::new(r"meldung::all\(").expect("pattern"), outside) > 0;
    let lookup = Regex::new(r"meldepflichten_for\(\s*([^)]*)\)").expect("pattern");
    let mut findings = Vec::new();
    let mut read: BTreeSet<String> = BTreeSet::new();
    for (path, text) in corpus.files.iter().filter(|(p, _)| outside(p)) {
        for c in lookup.captures_iter(text) {
            let arg = pid(&c[1]);
            if arg.chars().all(|ch| ch.is_ascii_digit()) && !arg.is_empty() {
                read.insert(arg);
            } else {
                findings.push(format!(
                    "{path}: `meldepflichten_for({})` — name the trigger PID literally, so the \
                     rows it reads can be counted",
                    &c[1]
                ));
            }
        }
    }
    // A row the lookup returns and the code names as undecided —
    // `nicht_entschieden(m, <trigger>, <pid>, reason)` — is not consulted.
    let skip = Regex::new(r"nicht_entschieden\(\s*[^,]+,\s*([0-9_]+)\s*,\s*([0-9_]+)\s*,")
        .expect("pattern");
    let skipped: BTreeSet<(String, String)> = corpus
        .files
        .iter()
        .filter(|(p, _)| outside(p))
        .flat_map(|(_, text)| {
            skip.captures_iter(text)
                .map(|c| (pid(&c[1]), pid(&c[2])))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut all: BTreeSet<String> = BTreeSet::new();
    let mut unread: BTreeSet<String> = BTreeSet::new();
    for (name, triggers) in &rows {
        all.insert(name.clone());
        let row_pid = name.split('←').next().unwrap_or_default().to_owned();
        let consulted = triggers
            .iter()
            .any(|t| read.contains(t) && !skipped.contains(&(t.clone(), row_pid.clone())));
        if !reads_all && !consulted {
            unread.insert(name.clone());
        }
    }
    // A step owed on silence is one named constant, read where it is named.
    let ohne_antwort = Regex::new(
        r"pub const (\w+): AusbleibendeAntwort = AusbleibendeAntwort \{\s*pid:\s*([0-9_]+)",
    )
    .expect("pattern");
    for c in ohne_antwort.captures_iter(&src) {
        let name = pid(&c[2]);
        all.insert(name.clone());
        let named = Regex::new(&format!(r"\b{}\b", &c[1])).expect("pattern");
        if corpus.count(&named, outside) == 0 {
            unread.insert(name);
        }
    }
    findings.extend(reconcile("Meldepflicht row", &unread, &all, MELDUNG_UNREAD));
    findings
}

// ── Credential fields ────────────────────────────────────────────────────────

fn credential_fields(root: &Path, corpus: &Corpus, services: &[PathBuf]) -> Vec<Finding> {
    let field = Regex::new(
        r"(?m)^\s*pub ([a-z_]*(?:secret|password|token|api_key|private_key|key_pem)[a-z_]*)\s*:\s*[^,\n]*(?:String|Secret)",
    )
    .expect("pattern");
    let mut all = BTreeSet::new();
    let mut unread = BTreeSet::new();
    let mut owners: Vec<(String, PathBuf)> = services
        .iter()
        .map(|s| (rel(root, s), s.join("src/config.rs")))
        .collect();
    owners.push((
        "crates/mako-service".into(),
        root.join("crates/mako-service/src/mcp_auth.rs"),
    ));
    for (owner, file) in owners {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let short = owner.rsplit('/').next().unwrap_or(&owner).to_owned();
        let prefix = format!("{owner}/src/");
        let names: Vec<String> = field.captures_iter(&src).map(|c| c[1].to_owned()).collect();
        for name in &names {
            let key = format!("{short}/{name}");
            if !all.insert(key.clone()) {
                continue;
            }
            let defs = names.iter().filter(|n| *n == name).count();
            let re = Regex::new(&format!(r"\b{name}\b")).expect("field");
            if corpus.count(&re, |p| p.starts_with(&prefix)) <= defs {
                unread.insert(key);
            }
        }
    }
    reconcile("credential field", &unread, &all, CREDENTIAL_UNREAD)
}

// ── Role columns ─────────────────────────────────────────────────────────────

fn role_columns(root: &Path, services: &[PathBuf]) -> Vec<Finding> {
    let col = Regex::new(
        r"(?m)^\s+((?:[a-z_]+_)?(?:rolle|role|roles|marktrolle|marktrole))\s+(?:TEXT|VARCHAR|CHAR)",
    )
    .expect("pattern");
    let mut out = Vec::new();
    let mut found = BTreeSet::new();
    for s in services {
        let svc = s
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned();
        let mut migrations = Vec::new();
        collect_sql(&s.join("migrations"), &mut migrations);
        for m in migrations {
            let Ok(sql) = std::fs::read_to_string(&m) else {
                continue;
            };
            for c in col.captures_iter(&sql) {
                found.insert((svc.clone(), c[1].to_owned()));
            }
        }
    }
    for (svc, column) in &found {
        if !ROLE_COLUMNS.iter().any(|(s, c, _)| s == svc && c == column) {
            out.push(format!(
                "role column `{svc}.{column}` names no consulting site — add it to ROLE_COLUMNS"
            ));
        }
    }
    for (svc, column, site) in ROLE_COLUMNS {
        if !found.contains(&((*svc).to_owned(), (*column).to_owned())) {
            out.push(format!(
                "role column `{svc}.{column}` is listed but not in any migration"
            ));
        }
        out.extend(check_site(
            root,
            &format!("role column `{svc}.{column}`"),
            site,
        ));
    }
    out
}

fn collect_sql(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "sql") {
            out.push(p);
        }
    }
}

fn check_site(root: &Path, what: &str, site: &Site) -> Vec<Finding> {
    match site {
        Site::At(path, needle) => match std::fs::read_to_string(root.join(path)) {
            Ok(src) if src.contains(needle) => Vec::new(),
            Ok(_) => vec![format!(
                "{what}: `{path}` no longer contains `{needle}` — is it still read?"
            )],
            Err(_) => vec![format!("{what}: consulting site `{path}` does not exist")],
        },
    }
}

// ── Feature-gated security modules ───────────────────────────────────────────

fn feature_gates(root: &Path, corpus: &Corpus) -> Vec<Finding> {
    let gate = Regex::new(
        r#"#\[cfg\([^\]]*feature\s*=\s*"[a-z0-9_-]+"[^\]]*\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([a-z]+\))? )?mod ([a-z_]*(?:auth|secur|cedar|oidc|verif|sign|tls|crypt|hmac|jwt|token|polic)[a-z_]*)\b"#,
    )
    .expect("pattern");
    let mut found = BTreeSet::new();
    for (path, src) in &corpus.files {
        let Some(krate) = path.split("/src/").next() else {
            continue;
        };
        for c in gate.captures_iter(src) {
            found.insert((krate.to_owned(), c[1].to_owned()));
        }
    }
    let mut out = Vec::new();
    for (krate, module) in &found {
        if !FEATURE_GATES
            .iter()
            .any(|(k, m, _)| k == krate && m == module)
        {
            out.push(format!(
                "feature-gated security module `{krate}::{module}` names no Cargo.toml that \
                 enables it — add it to FEATURE_GATES"
            ));
        }
    }
    for (krate, module, site) in FEATURE_GATES {
        if !found.contains(&((*krate).to_owned(), (*module).to_owned())) {
            out.push(format!(
                "feature gate `{krate}::{module}` is listed but no longer gated"
            ));
        }
        out.extend(check_site(
            root,
            &format!("feature gate `{krate}::{module}`"),
            site,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The findings about a planted fixture: a fixture tree carries none of
    /// the live lists' controls, so their entries are not the subject here.
    fn planted(found: Vec<Finding>) -> Vec<Finding> {
        found
            .into_iter()
            .filter(|f| f.contains("read by nothing") || f.contains("names no"))
            .collect()
    }

    fn tree(files: &[(&str, &str)]) -> tempdir::Dir {
        tempdir::Dir::new(files)
    }

    /// A throwaway workspace.
    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new(files: &[(&str, &str)]) -> Self {
                // A counter, not the clock: two tests running in parallel can
                // read the same nanosecond, share one directory and delete it
                // under each other.
                static NEXT: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let root = std::env::temp_dir().join(format!("ccc-{}-{n}", std::process::id()));
                for (p, s) in files {
                    let f = root.join(p);
                    std::fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
                    std::fs::write(f, s).expect("write");
                }
                Self(root)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    const MELDUNG: (&str, &str) = (
        "crates/mako-fristen/src/meldung.rs",
        "pub const GPKE: &[Meldepflicht] = &[Meldepflicht { pid: 99_999, triggered_by: &[11_111] }];\n",
    );
    const OHNE_ANTWORT: (&str, &str) = (
        "crates/mako-fristen/src/meldung.rs",
        "pub const STILLE: AusbleibendeAntwort = AusbleibendeAntwort { pid: 88_888 };\n",
    );
    const MELDUNG_READER: (&str, &str) = (
        "services/makod/src/meldung_reader.rs",
        "fn f() { for m in mako_fristen::meldung::meldepflichten_for(11_111) {} }",
    );

    fn corpus_of(t: &tempdir::Dir) -> Corpus {
        Corpus::load(
            &t.0,
            &dirs(&t.0.join("crates")),
            &dirs(&t.0.join("services")),
        )
    }

    #[test]
    fn an_unread_werktage_constant_fails() {
        let t = tree(&[(
            "crates/x/src/lib.rs",
            "pub const NEUE_ANTWORT_WERKTAGE: u32 = 5;\n",
        )]);
        let f = planted(frist_constants(&corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("NEUE_ANTWORT_WERKTAGE"));
    }

    #[test]
    fn a_read_constant_passes_and_a_test_only_reader_does_not_count() {
        let t = tree(&[
            (
                "crates/x/src/lib.rs",
                "pub const A_WERKTAGE: u32 = 5;\npub const B_WERKTAGE: u32 = 3;\n#[cfg(test)]\nmod t { const _: u32 = B_WERKTAGE; }\n",
            ),
            (
                "services/y/src/main.rs",
                "fn f() -> u32 { x::A_WERKTAGE }\n",
            ),
        ]);
        let f = planted(frist_constants(&corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("B_WERKTAGE"));
    }

    #[test]
    fn an_unread_meldepflicht_row_fails_until_something_reads_the_catalogue() {
        let t = tree(&[MELDUNG]);
        assert_eq!(planted(meldung_rows(&t.0, &corpus_of(&t))).len(), 1);
        let t = tree(&[MELDUNG, MELDUNG_READER]);
        assert!(planted(meldung_rows(&t.0, &corpus_of(&t))).is_empty());
    }

    /// A row is read by a lookup naming one of its own triggers — the same PID
    /// opened by another Sequenzdiagramm stays unread — and a lookup whose
    /// trigger is not a literal cannot be counted, so it is a finding.
    #[test]
    fn a_meldepflicht_row_is_read_only_through_its_own_trigger() {
        let other = (
            "services/makod/src/other.rs",
            "fn f() { for m in mako_fristen::meldung::meldepflichten_for(22_222) {} }",
        );
        let t = tree(&[MELDUNG, other]);
        let f = planted(meldung_rows(&t.0, &corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("99999←11111"), "{f:?}");
        let variable = (
            "services/makod/src/variable.rs",
            "fn f(p: u32) { for m in mako_fristen::meldung::meldepflichten_for(p) {} }",
        );
        let t = tree(&[MELDUNG, variable]);
        assert!(
            meldung_rows(&t.0, &corpus_of(&t))
                .iter()
                .any(|f| f.contains("name the trigger PID literally")),
        );
    }

    /// A row the lookup returns and the code names `nicht_entschieden` is a
    /// deliberate skip, not a reading; the other rows of that lookup are read.
    #[test]
    fn a_row_named_undecided_is_not_consulted() {
        const TWO: (&str, &str) = (
            "crates/mako-fristen/src/meldung.rs",
            "pub const G: &[Meldepflicht] = &[Meldepflicht { pid: 99_999, triggered_by: &[11_111] }, Meldepflicht { pid: 88_888, triggered_by: &[11_111] }];\n",
        );
        let reader = (
            "services/makod/src/reader.rs",
            "fn f() { for m in mako_fristen::meldung::meldepflichten_for(11_111) { if mako_fristen::meldung::nicht_entschieden(m, 11_111, 88_888, \"no fact\") { continue; } } }",
        );
        let t = tree(&[TWO, reader]);
        let f = planted(meldung_rows(&t.0, &corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("88888←11111"), "{f:?}");
    }

    /// A step owed on silence is read where its constant is named — not by a
    /// reader of the Meldepflicht catalogue, nor by its own test.
    #[test]
    fn a_step_owed_on_silence_is_read_where_its_constant_is_named() {
        let t = tree(&[OHNE_ANTWORT, MELDUNG_READER]);
        let f = planted(meldung_rows(&t.0, &corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("88888"), "{f:?}");
        let t = tree(&[
            OHNE_ANTWORT,
            (
                "services/makod/src/door.rs",
                "fn f() { mako_fristen::meldung::STILLE.window(); }\n",
            ),
        ]);
        assert!(planted(meldung_rows(&t.0, &corpus_of(&t))).is_empty());
    }

    /// A role column no query reads fails; the live sibling beside it does not
    /// hide it.
    #[test]
    fn an_unread_role_column_fails_beside_a_read_sibling() {
        let t = tree(&[(
            "services/kundend/migrations/0001.sql",
            "CREATE TABLE p (\n    standort_filter TEXT,\n    rolle           TEXT NOT NULL\n);\n",
        )]);
        let f = planted(role_columns(&t.0, &dirs(&t.0.join("services"))));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("kundend.rolle"));
    }

    #[test]
    fn an_unlisted_feature_gated_security_module_fails() {
        let t = tree(&[(
            "crates/x/src/lib.rs",
            "#[cfg(feature = \"strict\")]\npub mod signature_check;\n#[cfg(feature = \"tracing\")]\nmod spans;\n",
        )]);
        let f = planted(feature_gates(&t.0, &corpus_of(&t)));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("signature_check"));
    }

    #[test]
    fn an_unread_credential_field_fails() {
        let t = tree(&[
            (
                "services/z/src/config.rs",
                "pub struct C {\n    pub erp_hmac_secret: Option<String>,\n    pub api_key: String,\n}\n",
            ),
            (
                "services/z/src/main.rs",
                "fn f(c: &C) -> &str { &c.api_key }\n",
            ),
        ]);
        let f = planted(credential_fields(
            &t.0,
            &corpus_of(&t),
            &dirs(&t.0.join("services")),
        ));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("z/erp_hmac_secret"));
    }

    #[test]
    fn an_exemption_whose_control_gained_a_reader_fails() {
        let all: BTreeSet<String> = ["X_WERKTAGE".to_owned()].into();
        let list = [Unread {
            name: "X_WERKTAGE",
            reason: "r",
            milestone: "M1",
        }];
        assert_eq!(
            reconcile("Frist constant", &BTreeSet::new(), &all, &list).len(),
            1
        );
        assert!(reconcile("Frist constant", &all, &all, &list).is_empty());
    }

    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent/mako/workspace/root")));
    }
}
