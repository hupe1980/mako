//! Guard: every workflow command variant is constructed by shipped code.
//!
//! A command variant is the only way into the states its arm of `handle`
//! leads to. When nothing outside a test ever builds it, those states are
//! reachable in the test suite and nowhere else: the tests pass, the variant
//! looks wired, and production can never send the message or record the step.
//! A Redispatch activation whose `SendAcr` no door constructs never sends its
//! acknowledgement, however green its unit tests are.
//!
//! For every `type Command = X;` in `crates/*/src`, the guard parses the
//! variants of `enum X` and looks for a **construction** of each in shipped
//! code — `crates/*/src` and `services/*/src`, `#[cfg(test)]` items, comments
//! and string literals blanked. A construction is `X::Variant` (or
//! `Self::Variant` in the file that defines `X`, or an `as` alias of `X`) in
//! expression position; a match pattern — followed by `=>`, `|`, `if`, `@` or a
//! `let`-binding `=`, or preceded by `|` — does not count.
//!
//! A variant no shipped code constructs is refused unless [`EXEMPT`] names it
//! with the reason it legitimately has no constructor. An exemption naming a
//! variant that is constructed, or does not exist, is refused too, so the
//! list only shrinks with the code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::check_workflow_purity::{
    blank_cfg_test, code_mask, is_word_end, is_word_start, match_brace, walk_rs,
};

/// Command variants with no shipped constructor, and why that is correct.
///
/// `(enum name, variant, reason)`. Keep the reason a present-tense statement
/// of why no door builds it.
const EXEMPT: &[(&str, &str, &str)] = &[];

/// Command variants no shipped code constructs, held as a burn-down list.
///
/// Every state these variants lead to is reachable in tests only. The list only
/// shrinks: a variant not on it is refused, an entry whose variant gains a
/// constructor is refused, and so is one whose variant no longer exists — so
/// wiring a door or deleting a variant forces the entry out in the same change.
const UNWIRED: &[(&str, &str)] = &[
    ("AckForwardCommand", "Acknowledge"),
    ("AckForwardCommand", "Forward"),
    ("AktivierungCommand", "CascadeToAnb"),
    ("AktivierungCommand", "SendAcr"),
    ("AktivierungCommand", "SendAar"),
    ("AktivierungCommand", "RecordAnbResponse"),
    ("AktivierungCommand", "ForwardResponse"),
    ("AllokationslisteCommand", "SendAnforderung"),
    ("AnforderungCommand", "SendAnforderung"),
    ("AnfrageBestellungCommand", "DispatchResponse"),
    ("AnkuendigungZuordnungLfCommand", "ZuordnungBestaetigen"),
    ("BeendigungZuordnungCommand", "BeendenBestaetigen"),
    ("DatenabrufCommand", "SendAnfrage"),
    ("DatenabrufCommand", "NotifyDatenGeliefert"),
    ("DeviceChangeCommand", "Complete"),
    ("ErsteinbauCommand", "SendVorabinformation"),
    ("ErsteinbauCommand", "MeldeScheitern"),
    ("GasSperrungLfCommand", "InitiateSperrung"),
    ("GasSperrungLfCommand", "SendStornierung"),
    ("GasSperrungNbCommand", "BestaetigueSperrung"),
    ("GasStammdatenCommand", "SendAntwort"),
    ("GasStammdatenCommand", "SendAenderung"),
    ("GasSupplierChangeCommand", "ReceiveGnbAntwort"),
    ("GasSupplierChangeCommand", "Activate"),
    ("GeliGasDatenabrufCommand", "NotifyDatenGeliefert"),
    ("GeliGasLfAnmeldungCommand", "Activate"),
    ("GeliGasStornierungCommand", "DispatchAperak"),
    ("GeraeteubernahmeCommand", "DispatchAntwort"),
    ("GeraeteubernahmeCommand", "ConfirmTransfer"),
    ("GpkeStornierungCommand", "DispatchAperak"),
    ("KonfigurationAenderungCommand", "SendAnfrage"),
    ("KonfigurationCommand", "NbSendsBeauftragung"),
    ("KuendigungCommand", "BeendenBestaetigen"),
    ("LfAbmeldungCommand", "BeendenBestaetigen"),
    ("ModellwechselCommand", "Senden"),
    ("ModellwechselCommand", "SendAntwort"),
    ("NeuanlageCommand", "Aktivieren"),
    ("PreisanfrageCommand", "Abschliessen"),
    ("PreislisteCommand", "Aufzeichnen"),
    ("ProfilCommand", "Akzeptieren"),
    ("ProfilCommand", "SendReklamation"),
    ("StammdatenCommand", "SendAcknowledgement"),
    ("StammdatenCommand", "Forward"),
    ("StoerungsmeldungCommand", "SendStoerungsmeldung"),
    ("StoerungsmeldungCommand", "DispatchInformationsmeldung"),
    ("SupplierChangeCommand", "Activate"),
    ("TechnikAenderungCommand", "SendAuftrag"),
    ("WertebestellungCommand", "MarkLieferungBegonnen"),
    ("WertebestellungCommand", "BeendenDurchMsb"),
    ("ZpLifecycleCommand", "SendAnfrage"),
    ("ZpLifecycleCommand", "SendAntwort"),
    ("ZpLifecycleCommand", "SendWeiterleitung"),
];

/// The command enums the tree holds — the measured value, not a margin below
/// it. Fewer means the
/// `type Command = …` shape moved and the guard must be taught the new one.
const MIN_COMMAND_ENUMS: usize = 57;

/// One command enum: where it is defined and its variants.
#[derive(Debug)]
struct CommandEnum {
    file: String,
    variants: Vec<String>,
}

/// Run the check from `workspace_root`. Returns `true` when clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut crate_files: Vec<PathBuf> = Vec::new();
    let mut service_files: Vec<PathBuf> = Vec::new();
    for (tree, out) in [
        ("crates", &mut crate_files),
        ("services", &mut service_files),
    ] {
        let Ok(entries) = std::fs::read_dir(workspace_root.join(tree)) else {
            eprintln!("check-command-constructors: cannot read {tree}/");
            return false;
        };
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                walk_rs(&src, out);
            }
        }
    }
    let read = |p: &PathBuf| -> (String, String) {
        let rel = p
            .strip_prefix(workspace_root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        (rel, std::fs::read_to_string(p).unwrap_or_default())
    };
    let crates: Vec<(String, String)> = crate_files.iter().map(read).collect();
    let services: Vec<(String, String)> = service_files.iter().map(read).collect();

    let enums = command_enums(&crates);
    if enums.len() < MIN_COMMAND_ENUMS {
        eprintln!(
            "check-command-constructors: found {} command enums, expected at least \
             {MIN_COMMAND_ENUMS} — refusing to report a clean tree it did not read",
            enums.len()
        );
        return false;
    }

    let constructed = constructed_variants(&enums, crates.iter().chain(services.iter()));
    let exempt: Vec<(&str, &str, &str)> = EXEMPT
        .iter()
        .copied()
        .chain(UNWIRED.iter().map(|&(e, v)| (e, v, "unwired")))
        .collect();
    let findings = verdict(&enums, &constructed, &exempt);
    let total: usize = enums.values().map(|e| e.variants.len()).sum();
    if findings.is_empty() {
        println!(
            "check-command-constructors: OK — {} command enums, {total} variants, {} exempt, \
             {} unwired",
            enums.len(),
            EXEMPT.len(),
            UNWIRED.len()
        );
        return true;
    }
    eprintln!("check-command-constructors: {} finding(s)", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nA command variant nothing ships a constructor for leads to states only a test \
         can reach. Wire it to the door that sends or receives its message, delete it with \
         the states only it reaches, or name it in EXEMPT with the reason. UNWIRED only \
         shrinks."
    );
    false
}

/// Every command enum named by a `type Command = X;`, keyed by `X`.
fn command_enums(files: &[(String, String)]) -> BTreeMap<String, CommandEnum> {
    let mut names = BTreeSet::new();
    for (_, src) in files {
        let mut mask = code_mask(src);
        blank_cfg_test(src, &mut mask);
        for (at, _) in src.match_indices("type Command") {
            if !mask[at] {
                continue;
            }
            let rest = src[at + "type Command".len()..].trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let path: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                .collect();
            if let Some(name) = path.rsplit("::").next().filter(|n| !n.is_empty()) {
                names.insert(name.to_owned());
            }
        }
    }
    let mut out = BTreeMap::new();
    for name in names {
        for (rel, src) in files {
            if let Some(variants) = enum_variants(src, &name) {
                out.insert(
                    name.clone(),
                    CommandEnum {
                        file: rel.clone(),
                        variants,
                    },
                );
                break;
            }
        }
    }
    out
}

/// The variants of `enum name` defined in `src`, if it is defined there.
fn enum_variants(src: &str, name: &str) -> Option<Vec<String>> {
    let mut mask = code_mask(src);
    blank_cfg_test(src, &mut mask);
    let bytes = src.as_bytes();
    let needle = format!("enum {name}");
    let at = src
        .match_indices(&needle)
        .map(|(i, _)| i)
        .find(|&i| mask[i] && is_word_start(bytes, i) && is_word_end(bytes, i + needle.len()))?;
    let open = (at..src.len()).find(|&i| mask[i] && bytes[i] == b'{')?;
    let close = match_brace(bytes, &mask, open)?;
    let mut variants = Vec::new();
    let mut depth = 0usize;
    let mut expect_name = true;
    let mut i = open + 1;
    while i < close {
        let b = bytes[i];
        if !mask[i] {
            i += 1;
            continue;
        }
        match b {
            b'{' | b'(' | b'[' | b'<' => depth += 1,
            b'}' | b')' | b']' | b'>' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => expect_name = true,
            b'#' if depth == 0 => {
                // Skip an attribute `#[…]` wholesale.
                if let Some(j) = (i..close).find(|&j| bytes[j] == b'[') {
                    let mut d = 0usize;
                    let mut k = j;
                    while k < close {
                        match bytes[k] {
                            b'[' => d += 1,
                            b']' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    i = k + 1;
                    continue;
                }
            }
            c if depth == 0 && expect_name && (c.is_ascii_alphabetic() || c == b'_') => {
                let ident: String = src[i..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                i += ident.len();
                variants.push(ident);
                expect_name = false;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    Some(variants)
}

/// `(enum, variant)` pairs constructed somewhere in `files`.
fn constructed_variants<'a>(
    enums: &BTreeMap<String, CommandEnum>,
    files: impl Iterator<Item = &'a (String, String)> + Clone,
) -> BTreeSet<(String, String)> {
    // `use … X as Y` anywhere — a crate's `pub use` re-export included — names
    // `X` as `Y` for every file that imports it.
    let mut aliases: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (_, src) in files.clone() {
        let mask = code_mask(src);
        for name in enums.keys() {
            let alias_of = format!("{name} as ");
            for (at, _) in src.match_indices(&alias_of) {
                if mask[at] && is_word_start(src.as_bytes(), at) {
                    let alias: String = src[at + alias_of.len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !alias.is_empty() && alias != "_" {
                        aliases.entry(name.as_str()).or_default().insert(alias);
                    }
                }
            }
        }
    }

    let mut out = BTreeSet::new();
    for (rel, src) in files {
        let mut mask = code_mask(src);
        blank_cfg_test(src, &mut mask);
        let bytes = src.as_bytes();
        for (name, def) in enums {
            // The spellings that name this enum in this file.
            let mut heads: Vec<&str> = vec![name.as_str()];
            if def.file == *rel {
                heads.push("Self");
            }
            if let Some(a) = aliases.get(name.as_str()) {
                heads.extend(a.iter().map(String::as_str));
            }
            for head in heads {
                let needle = format!("{head}::");
                for (at, _) in src.match_indices(&needle) {
                    if !mask[at] || !is_word_start(bytes, at) {
                        continue;
                    }
                    let vstart = at + needle.len();
                    let variant: String = src[vstart..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !def.variants.contains(&variant) {
                        continue;
                    }
                    if is_construction(src, &mask, at, vstart + variant.len()) {
                        out.insert((name.clone(), variant));
                    }
                }
            }
        }
    }
    out
}

/// Is the path `src[start..end]` (`X::Variant`) used as a value rather than a
/// pattern?
fn is_construction(src: &str, mask: &[bool], start: usize, end: usize) -> bool {
    let bytes = src.as_bytes();
    // Skip a `{ … }` or `( … )` payload.
    let mut i = end;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'{' || bytes[i] == b'(') {
        let (open, close) = (bytes[i], if bytes[i] == b'{' { b'}' } else { b')' });
        let mut depth = 0usize;
        while i < bytes.len() {
            if mask[i] {
                if bytes[i] == open {
                    depth += 1;
                } else if bytes[i] == close {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
            }
            i += 1;
        }
    }
    let after = src[i.min(src.len())..].trim_start();
    if after.starts_with("=>")
        || (after.starts_with('|') && !after.starts_with("||"))
        || after.starts_with("if ")
        || after.starts_with('@')
        || (after.starts_with('=') && !after.starts_with("=="))
    {
        return false;
    }
    let before = src[..start].trim_end();
    if before.ends_with('|') && !before.ends_with("||") {
        return false;
    }
    // `matches!(value, X::V { .. })`: the path is the macro's second argument.
    let stmt_start = before.rfind([';', '{', '}']).map_or(0, |p| p + 1);
    let stmt = &before[stmt_start..];
    if let Some(m) = stmt.rfind("matches!(") {
        let inside = &stmt[m..];
        let open_parens = inside.matches('(').count();
        let close_parens = inside.matches(')').count();
        if open_parens > close_parens {
            return false;
        }
    }
    true
}

/// The findings for a set of enums, constructions and exemptions.
fn verdict(
    enums: &BTreeMap<String, CommandEnum>,
    constructed: &BTreeSet<(String, String)>,
    exempt: &[(&str, &str, &str)],
) -> Vec<String> {
    let mut findings = Vec::new();
    for (name, def) in enums {
        for v in &def.variants {
            let built = constructed.contains(&(name.clone(), v.clone()));
            let exempted = exempt.iter().any(|(e, ev, _)| e == name && ev == v);
            if !built && !exempted {
                findings.push(format!(
                    "{}: `{name}::{v}` has no constructor in shipped code",
                    def.file
                ));
            }
            if built && exempted {
                findings.push(format!(
                    "{}: `{name}::{v}` is constructed — remove its EXEMPT/UNWIRED entry",
                    def.file
                ));
            }
        }
    }
    for (e, ev, _) in exempt {
        let exists = enums
            .get(*e)
            .is_some_and(|d| d.variants.iter().any(|v| v == ev));
        if !exists {
            findings.push(format!(
                "EXEMPT/UNWIRED names `{e}::{ev}`, which no longer exists"
            ));
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = r#"
pub enum PingCommand {
    /// Sent by the door.
    Send { to: String },
    #[serde(rename = "x")]
    Receive(u32),
    TimeoutExpired { deadline_id: DeadlineId, label: Box<str> },
}
impl Workflow for PingWorkflow {
    type Command = PingCommand;
    fn on_deadline(d: &Deadline, s: &S) -> Option<PingCommand> {
        Some(PingCommand::TimeoutExpired { deadline_id: d.deadline_id(), label: "x".into() })
    }
    fn handle(s: &S, c: PingCommand) -> R {
        match c {
            PingCommand::Send { to } => todo!(),
            PingCommand::Receive(_) | Self::TimeoutExpired { .. } => todo!(),
        }
    }
}
"#;

    fn scan(door: &str) -> Vec<String> {
        let files = [
            ("crates/ping/src/lib.rs".to_owned(), DEF.to_owned()),
            ("services/makod/src/door.rs".to_owned(), door.to_owned()),
        ];
        let enums = command_enums(&files[..1]);
        let constructed = constructed_variants(&enums, files.iter());
        verdict(&enums, &constructed, &[])
    }

    #[test]
    fn the_variants_are_parsed_past_attributes_and_payloads() {
        let files = [("crates/ping/src/lib.rs".to_owned(), DEF.to_owned())];
        let enums = command_enums(&files);
        assert_eq!(
            enums["PingCommand"].variants,
            ["Send", "Receive", "TimeoutExpired"]
        );
    }

    #[test]
    fn a_variant_constructed_only_in_a_test_is_refused() {
        let findings = scan(
            r#"
async fn door() { p.run(PingCommand::Send { to: x }, o).await; }
#[cfg(test)]
mod tests { fn t() { let _ = PingCommand::Receive(1); } }
"#,
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("PingCommand::Receive"), "{findings:?}");
    }

    #[test]
    fn match_arms_and_matches_are_not_constructions() {
        let findings = scan(
            r#"
async fn door() {
    p.run(PingCommand::Send { to: x }, o).await;
    if let PingCommand::Receive(n) = c {}
    assert!(matches!(c, PingCommand::Receive(_)));
}
"#,
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn every_variant_constructed_is_clean() {
        let findings = scan(
            r#"
async fn door() {
    p.run(PingCommand::Send { to: x }, o).await;
    let cmd = PingCommand::Receive(7);
}
"#,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_stale_exemption_is_refused() {
        let files = [("crates/ping/src/lib.rs".to_owned(), DEF.to_owned())];
        let enums = command_enums(&files);
        let constructed = constructed_variants(&enums, files.iter());
        let findings = verdict(
            &enums,
            &constructed,
            &[
                ("PingCommand", "TimeoutExpired", "built by on_deadline"),
                ("PingCommand", "Gone", "deleted"),
            ],
        );
        assert!(
            findings
                .iter()
                .any(|f| f.contains("remove its EXEMPT/UNWIRED entry")),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("no longer exists")),
            "{findings:?}"
        );
    }
}
