//! `cargo xtask validate-ebd-codes` — hold `mako-pruefung`'s Antwortcode
//! catalogue against the BDEW *Entscheidungsbaum-Diagramme und Codelisten* PDF.
//!
//! # Why this check exists
//!
//! A tree's answer code is two facts, not one: the code itself and the
//! **Cluster** that says whether it agrees or refuses. The same code means
//! opposite things in trees that run in the *same* process — `A01` is an
//! Ablehnung in `E_0510` and a Zustimmung in `E_0511`/`E_0512`, and `E_0205`
//! and `E_0208` overlap on `A01`–`A03` while disagreeing about what each means.
//! A catalogue entry that names the wrong Cluster therefore answers a
//! confirmation with a refusal, on the wire, silently: the code is valid, the
//! tree publishes it, and every existing guard passes.
//!
//! Nothing else in the tree compares the catalogue to the document it was
//! transcribed from. `validate-pruefids` checks that a PID resolves to a tree
//! and `validate-release-codes` that a profile has a fixture; both take the
//! catalogue's contents on trust.
//!
//! # What it checks
//!
//! For every `code!(…)` entry in `crates/mako-pruefung/src`:
//!
//! 1. the tree it names is a heading in the EBD document;
//! 2. the tree publishes that code;
//! 3. the Cluster mako assigns is the Cluster the document prints.
//!
//! A tree the document annotates with no `Cluster:` at all — the Codelisten
//! tables of WiM Kap. 14, which print `Code / Nutzung / Name` instead of
//! Prüfschritte — is checked for (1) and (2) only, and counted separately.
//!
//! And **the other direction**: every code the document publishes for a tree
//! must be in mako's catalogue, or listed in [`KNOWN_UNCARRIED`] with a reason.
//!
//! The three checks above can only be as complete as the catalogue is, so a
//! whole *branch* of a tree can be absent without producing a disagreement.
//! Several trees split on their first Prüfschritt into branches that share no
//! Antwortcode — not even the Zustimmung — and a catalogue carrying one of them
//! answers the other from the wrong alphabet, on the happy path.
//!
//! # Source-gated, like every other regulatory check
//!
//! `regulatories/` is gitignored, so the PDF is absent in a fresh checkout and
//! on CI. This check then **skips** rather than failing: it can only be as
//! present as the document is. Run `cargo xtask sync-regulatories --download`
//! to fetch it. Text extraction needs poppler's `pdftotext`; without it the
//! check skips for the same reason.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Where the mirrored BDEW documents live.
const MIRROR_DIR: &str = "regulatories/bdew-mako";

/// The catalogue's source, as `mako-pruefung`'s module docs cite it.
const EBD_STEM: &str = "Entscheidungsbaum-Diagramme_und_Codelisten";

/// `Cluster` variant → the wording the EBD prints after `Cluster:`.
///
/// The document's wording is the authority; the Rust identifier is mako's. A
/// variant missing here is compared by its own name, which fails loudly rather
/// than passing silently.
const CLUSTER_WORDING: &[(&str, &str)] = &[
    ("Zustimmung", "zustimmung"),
    ("Ablehnung", "ablehnung"),
    ("Abweisung", "abweisung"),
    ("AenderungDerDaten", "anderung der daten"),
    ("KeineAenderungDerDaten", "keine anderung der daten"),
    ("AblehnungDerGesamtenListe", "ablehnung der gesamten liste"),
];

/// One catalogue entry, as written in the Rust source.
struct Entry {
    tree: String,
    code: String,
    cluster: String,
    file: String,
}

pub fn run(workspace_root: &str) -> bool {
    let root = Path::new(workspace_root);

    let Some(pdf) = newest_ebd(&root.join(MIRROR_DIR)) else {
        eprintln!(
            "validate-ebd-codes: SKIPPED — no {EBD_STEM}*.pdf under {MIRROR_DIR}. \
             Run `cargo xtask sync-regulatories --download` to fetch it."
        );
        return true;
    };
    let Some(text) = layout_text(&pdf) else {
        eprintln!(
            "validate-ebd-codes: SKIPPED — `pdftotext` (poppler) is not on PATH, \
             so {} cannot be read.",
            pdf.display()
        );
        return true;
    };

    let doc = parse_document(&text);
    let entries = parse_catalogue(&root.join("crates/mako-pruefung/src"));
    if entries.is_empty() {
        eprintln!("validate-ebd-codes: no `code!` entries found — the parser is broken");
        return false;
    }

    let mut errors: Vec<String> = Vec::new();
    let mut trees_seen: BTreeSet<&str> = BTreeSet::new();
    // The catalogue indexed the way the completeness check below reads it.
    let mut by_tree: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &entries {
        by_tree
            .entry(e.tree.as_str())
            .or_default()
            .insert(e.code.as_str());
    }
    let mut clustered = 0usize;
    let mut listed = 0usize;

    for e in &entries {
        trees_seen.insert(&e.tree);
        let Some(tree) = doc.get(&e.tree) else {
            errors.push(format!(
                "{}: tree {} is not a heading in {} — mako answers with a tree the \
                 document does not publish",
                e.file,
                e.tree,
                pdf.file_name().unwrap_or_default().to_string_lossy()
            ));
            continue;
        };
        if !tree.codes.contains(&e.code) {
            errors.push(format!(
                "{}: {} does not publish code {} — every code must come from the tree \
                 that answers with it",
                e.file, e.tree, e.code
            ));
            continue;
        }
        match tree.clusters.get(&e.code) {
            // The Codelisten tables print no Cluster; code membership is all
            // that can be checked there.
            None => listed += 1,
            Some(printed) => {
                clustered += 1;
                let want = CLUSTER_WORDING
                    .iter()
                    .find(|(id, _)| *id == e.cluster)
                    .map(|(_, w)| *w)
                    .unwrap_or(e.cluster.as_str());
                let got = normalise(printed);
                // The document qualifies some clusters („Ablehnung auf
                // Kopfebene"); the prefix is the cluster, the rest is the level.
                if !got.starts_with(want) {
                    errors.push(format!(
                        "{}: {} {} is Cluster::{} in mako but „{}\" in the EBD — the \
                         same code with the opposite meaning reaches the wire",
                        e.file, e.tree, e.code, e.cluster, printed
                    ));
                }
            }
        }
    }

    // ── The other direction: codes the document publishes and mako omits ─────
    //
    // The check above can only be as complete as the catalogue is, so a whole
    // branch of a tree can be missing without a single disagreement. `E_0607`
    // was: Prüfschritt 10 „nein" opens an erzeugende branch with six codes of
    // its own — including its **Zustimmung** — and mako carried none of them,
    // so an erzeugende Abmeldung was confirmed with the verbrauchend branch's
    // `A11`. Every existing guard passed.
    for (tree, mako_codes) in &by_tree {
        let Some(published) = doc.get(*tree) else {
            continue; // already reported above
        };
        let missing: Vec<&String> = published
            .codes
            .iter()
            .filter(|c| !mako_codes.contains(c.as_str()))
            .filter(|c| !is_exempt(tree, c))
            .collect();
        if !missing.is_empty() {
            errors.push(format!(
                "{tree}: the EBD publishes {} code(s) mako's catalogue omits — {}. A code \
                 mako does not carry is a branch mako cannot answer from, and it answers \
                 from the wrong one instead. Add them, or list them in KNOWN_UNCARRIED \
                 with the reason.",
                missing.len(),
                missing
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if errors.is_empty() {
        println!(
            "validate-ebd-codes: {} code(s) across {} tree(s) agree with {} \
             ({clustered} Cluster-annotated, {listed} from a Codelisten table)",
            entries.len(),
            trees_seen.len(),
            pdf.file_name().unwrap_or_default().to_string_lossy(),
        );
        return true;
    }
    for e in &errors {
        eprintln!("ERROR  {e}");
    }
    eprintln!(
        "\nvalidate-ebd-codes: {} disagreement(s) with the published EBD",
        errors.len()
    );
    false
}

/// Codes the completeness check must not report for a tree, with the reason.
///
/// Two kinds of entry, and they are not the same thing:
///
/// - **Scan noise.** A section's codes are every code-shaped token in its text,
///   so a Prüfschritt that *quotes* another tree's code („wurde der Code A30 …
///   verwendet?") contributes it, and a Codelisten table printed after the last
///   `E_####` heading of a chapter is attributed to that tree. Neither is an
///   omission.
/// - **A tracked gap.** `"*"` exempts the whole tree; use it only with a pointer
///   to where the work is recorded, never to quiet a finding.
const KNOWN_UNCARRIED: &[(&str, &str, &str)] = &[
    (
        "E_0623",
        "A30",
        "quoted by Prüfschritt 50: „wurde der Code A30 … verwendet?“ — an E_0624 code",
    ),
    (
        "E_0623",
        "A41",
        "quoted by Prüfschritt 440, the Tranchen twin of A30 — an E_0624 code",
    ),
    (
        "E_0275",
        "E15",
        "from the Codelisten table that follows chapter 10, not from this tree",
    ),
    ("E_0275", "Z13", "same table as E15"),
    ("E_0275", "Z15", "same table as E15"),
    ("E_0275", "Z21", "same table as E15"),
    (
        "E_0406",
        "*",
        "the Netznutzungsabrechnung tree has 205 Prüfschritte and mako answers with three \
         codes; closing it is a ROADMAP item („`E_0406` is answered with three codes out of \
         eighty-seven“), not an oversight",
    ),
];

/// Is this code exempt from the completeness check for this tree?
fn is_exempt(tree: &str, code: &str) -> bool {
    KNOWN_UNCARRIED
        .iter()
        .any(|(t, c, _)| *t == tree && (*c == code || *c == "*"))
}

// ── the document ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct Tree {
    /// Every code token appearing anywhere in the tree's section.
    codes: BTreeSet<String>,
    /// The Cluster printed for a code, where the tree prints one.
    clusters: BTreeMap<String, String>,
}

/// Split the extracted text into tree sections and read each one's codes.
///
/// Headings look like `17.1.3          E_0510_Anmeldung prüfen`. The table of
/// contents uses the same shape padded with dot leaders, so those lines are
/// dropped rather than parsed as sections.
///
/// # A tree's codes are often printed in a *separate* section
///
/// Only the Prüfschritt trees carry their codes inline. The WiM trees put
/// theirs in a `G_####` Codeliste beside the tree, and the document does it two
/// ways: nested (`14.7.1 E_2014` → `14.7.1.1 G_0083`) and as following siblings
/// (`14.3.7 E_2004` → `14.3.8 G_0072`, `14.3.9 G_0073`). Attributing those codes
/// to the `G_####` alone would report every one of them as unpublished by the
/// tree that actually answers with them.
///
/// So a section contributes its codes to itself, to every heading whose section
/// number is a dotted prefix of its own, and — when it is a `G_####` Codeliste —
/// to the nearest `E_####` tree heading before it, which is the tree it lists
/// codes for.
fn parse_document(text: &str) -> BTreeMap<String, Tree> {
    let lines: Vec<&str> = text.lines().collect();
    let heads: Vec<(usize, String, String)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.contains("...."))
        .filter_map(|(i, l)| heading(l).map(|(num, id)| (i, num, id)))
        .collect();

    // Every heading is a tree the document publishes, whether or not the
    // section prints a code table — a tree that decides purely by branching to
    // another one (`E_0513`) has no codes of its own, and must still be known.
    let mut out: BTreeMap<String, Tree> = BTreeMap::new();
    for (_, _, id) in &heads {
        out.entry(id.clone()).or_default();
    }
    for (n, (start, num, _)) in heads.iter().enumerate() {
        let end = heads.get(n + 1).map_or(lines.len(), |(i, ..)| *i);
        let (_, _, id) = &heads[n];
        // This section, every ancestor section by number, and — for a `G_####`
        // Codeliste — the tree it lists codes for.
        let mut owners: Vec<&String> = heads
            .iter()
            .filter(|(_, other, _)| is_ancestor_or_self(other, num))
            .map(|(_, _, oid)| oid)
            .collect();
        if id.starts_with('G') {
            if let Some((_, _, tree)) = heads[..n]
                .iter()
                .rev()
                .find(|(_, _, other)| other.starts_with('E'))
            {
                owners.push(tree);
            }
        }
        for (li, line) in lines[*start..end].iter().enumerate() {
            for tok in code_tokens(line) {
                // `Cluster:` sits in the Hinweis column — same line as the code,
                // or the next one when the row wraps.
                let cluster = cluster_after(line, &tok).or_else(|| {
                    lines
                        .get(*start + li + 1)
                        .and_then(|nx| cluster_at_start(nx))
                });
                for id in &owners {
                    let entry = out.entry((*id).clone()).or_default();
                    entry.codes.insert(tok.clone());
                    if let Some(c) = cluster.clone() {
                        entry.clusters.entry(tok.clone()).or_insert(c);
                    }
                }
            }
        }
    }
    out
}

/// `14.7.1` is an ancestor of `14.7.1.1`, and of itself. `14.7.10` is not.
fn is_ancestor_or_self(maybe_ancestor: &str, section: &str) -> bool {
    section == maybe_ancestor
        || (section.starts_with(maybe_ancestor)
            && section.as_bytes().get(maybe_ancestor.len()) == Some(&b'.'))
}

/// `12.3.4   E_0510_…` → `("12.3.4", "E_0510")`.
fn heading(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let mut it = t.split_whitespace();
    let num = it.next()?;
    if !num.starts_with(|c: char| c.is_ascii_digit())
        || !num.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return None;
    }
    // `E_0510_Anmeldung prüfen` — the id is the first six characters; the
    // underscore after them separates it from the tree's name, and splitting on
    // the *first* underscore would yield `E`.
    let word = it.next()?;
    if word.len() <= 6 || word.as_bytes()[6] != b'_' {
        return None;
    }
    let id = &word[..6];
    is_tree_id(id).then(|| (num.to_owned(), id.to_owned()))
}

/// `E_0510` / `G_0083` — one letter, an underscore, four digits.
fn is_tree_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 6
        && (b[0] == b'E' || b[0] == b'G')
        && b[1] == b'_'
        && b[2..].iter().all(u8::is_ascii_digit)
}

/// The answer-code tokens on one line.
///
/// Two code spaces appear: the `A##` of a Prüfschritt tree and the
/// `Code / Nutzung / Name` table of a Codeliste, whose codes are numeric or
/// `Z##`/`E##`. A Codeliste row is recognised by its Nutzung column — `O`, `M`
/// or `X`, depending on the table — so prose that happens to contain a number
/// is not mistaken for a code.
fn code_tokens(line: &str) -> Vec<String> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, w) in f.iter().enumerate() {
        let w = w.trim_end_matches(['.', ',']);
        // Either code space: an `A##` needs no context, a Codelisten code is
        // only a code when the Nutzung column follows it.
        let is_code = is_a_code(w)
            || (matches!(f.get(i + 1), Some(&"O") | Some(&"M") | Some(&"X")) && is_listed_code(w));
        if is_code {
            out.push(w.to_owned());
        }
    }
    out
}

/// `A01`, `A100`, `AC1` — the Prüfschritt code space.
fn is_a_code(w: &str) -> bool {
    let b = w.as_bytes();
    match b {
        [b'A', b'C', d] => d.is_ascii_digit(),
        [b'A', rest @ ..] if (2..=3).contains(&rest.len()) => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// A Codelisten code: `5`, `53`, `Z01`, `E15`, `ZB6`.
fn is_listed_code(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= 3
        && w.bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && w.bytes().any(|c| c.is_ascii_digit())
}

fn cluster_after(line: &str, code: &str) -> Option<String> {
    let at = line.find(code)?;
    cluster_value(&line[at + code.len()..])
}

fn cluster_at_start(line: &str) -> Option<String> {
    cluster_value(line)
}

fn cluster_value(s: &str) -> Option<String> {
    let rest = s.split_once("Cluster:")?.1.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

fn normalise(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace('ä', "a")
        .replace('ö', "o")
        .replace('ü', "u")
        .replace('ß', "ss")
}

// ── the catalogue ─────────────────────────────────────────────────────────────

/// Read every `code!("A01", E_0510, Ablehnung, …)` under `dir`.
///
/// The `E_0510` argument is a module-scoped `const … = Some(EBD_X)` alias, and
/// two modules use the *same* alias name for different trees
/// (`EBD_ABMELDUNG` is `E_0609` in `codes` and `E_0512` in `emob`), so
/// resolution is per file — exactly as Rust scopes it.
fn parse_catalogue(dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(src) = std::fs::read_to_string(&p) {
                    collect(&src, &p, &mut out);
                }
            }
        }
    }
    out
}

fn collect(src: &str, path: &Path, out: &mut Vec<Entry>) {
    // `pub const EBD_ABMELDUNG: &str = "E_0609";`
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    for (i, _) in src.match_indices("const ") {
        let line = &src[i..src[i..].find(';').map_or(src.len(), |e| i + e)];
        if let Some((decl, val)) = line.split_once('=') {
            if decl.contains(": &str") {
                if let (Some(n), Some(v)) = (ident_after(decl, "const "), quoted(val)) {
                    if is_tree_id(v) {
                        names.insert(n, v);
                    }
                }
            }
        }
    }
    // `const E_0609: Option<&'static str> = Some(EBD_ABMELDUNG);`
    let mut alias: BTreeMap<&str, &str> = BTreeMap::new();
    for (i, _) in src.match_indices("const ") {
        let line = &src[i..src[i..].find(';').map_or(src.len(), |e| i + e)];
        if !line.contains("Option<&'static str>") {
            continue;
        }
        let Some(n) = ident_after(line, "const ") else {
            continue;
        };
        let target = line
            .split_once("Some(")
            .and_then(|(_, r)| r.split_once(')'))
            .map(|(t, _)| t.trim());
        let resolved = target.and_then(|t| names.get(t).copied());
        if let Some(t) = resolved.or_else(|| is_tree_id(n).then_some(n)) {
            alias.insert(n, t);
        }
    }

    let file = path
        .strip_prefix(std::env::current_dir().unwrap_or_default())
        .unwrap_or(path)
        .display()
        .to_string();

    for (i, _) in src.match_indices("code!(") {
        let body = &src[i + "code!(".len()..];
        let Some(close) = body.find(')') else {
            continue;
        };
        let args: Vec<&str> = body[..close].split(',').map(str::trim).collect();
        if args.len() < 3 {
            continue;
        }
        let (Some(code), sym, cluster) = (quoted(args[0]), args[1], args[2]) else {
            continue;
        };
        let tree = alias
            .get(sym)
            .copied()
            .or_else(|| names.get(sym).copied())
            .or_else(|| is_tree_id(sym).then_some(sym));
        if let Some(tree) = tree {
            out.push(Entry {
                tree: tree.to_owned(),
                code: code.to_owned(),
                cluster: cluster.to_owned(),
                file: file.clone(),
            });
        }
    }
}

fn ident_after<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let r = s.split_once(kw)?.1.trim_start();
    let end = r.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    (end > 0).then(|| &r[..end])
}

fn quoted(s: &str) -> Option<&str> {
    let a = s.find('"')? + 1;
    let b = s[a..].find('"')? + a;
    Some(&s[a..b])
}

// ── the PDF ───────────────────────────────────────────────────────────────────

/// The newest mirrored EBD PDF, preferring a konsolidierte Lesefassung.
fn newest_ebd(dir: &Path) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "pdf")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(EBD_STEM))
        })
        .collect();
    // Lexicographic order puts a later version last, and a "konsolidierte
    // Lesefassung" after the bare release of the same number.
    hits.sort();
    hits.pop()
}

fn layout_text(pdf: &Path) -> Option<String> {
    let out = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(pdf)
        .arg("-")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
