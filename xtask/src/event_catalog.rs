//! `cargo xtask event-catalog [--check]`: the CloudEvent catalog page and the
//! published JSON Schema of each type's `data`.
//!
//! Generated from three sources, so the page cannot disagree with them:
//!
//! - `crates/mako-events/src/lib.rs` — every type, its first doc line, and
//!   whether it is a phantom (`⚠ phantom:`, no service emits it);
//! - the committed `events/<type>.json` of each crate that emits the type
//!   through a payload bound to it (`mako_service::EventPayload`), written and
//!   held by that crate's own test — the crate is the type's emitter;
//! - the production source of `services/*/src`, for the services that name
//!   the type's constant.
//!
//! It writes `site/content/docs/reference/events.md` and copies each schema to
//! `site/static/schemas/events/<type>.json`, the URL a typed event carries in
//! `dataschema`. `--check` refuses a page or a schema that differs.
//!
//! [`UNTYPED`] is a shrink-only ratchet: every non-phantom type not yet emitted
//! through a bound payload. A non-phantom type without a schema that is not
//! listed fails, and a listed type that has gained a schema (or become a
//! phantom, or left the catalog) fails until its entry is removed. It is empty
//! when every emitted type is bound to a payload.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const CATALOG: &str = "crates/mako-events/src/lib.rs";
const PAGE: &str = "site/content/docs/reference/events.md";
const SCHEMAS: &str = "site/static/schemas/events";
/// Where the site serves [`SCHEMAS`] — `mako_service::cloudevent::SCHEMA_BASE`.
const BASE: &str = "https://hupe1980.github.io/mako/schemas/events/";

/// Non-phantom types emitted without a payload bound to them.
const UNTYPED: &[&str] = &[
    "de.accounting.abschlag.posted",
    "de.accounting.abwendung.angeboten",
    "de.accounting.abwendung.gebrochen",
    "de.accounting.bankruecklast",
    "de.accounting.eeg.payout.rejected",
    "de.accounting.entsperrauftrag",
    "de.accounting.erstattung.faellig",
    "de.accounting.interest.charged",
    "de.accounting.jahresabschluss.abgeschlossen",
    "de.accounting.mahnung.issued",
    "de.accounting.payee.verification-mismatch",
    "de.accounting.payment.due",
    "de.accounting.payment.imported",
    "de.accounting.sepa.collection-rejected",
    "de.accounting.sepa.recall-requested",
    "de.accounting.sepa.recall-resolved",
    "de.accounting.sepa.reversal-issued",
    "de.accounting.sperrandrohung",
    "de.accounting.sperrankuendigung",
    "de.accounting.sperrauftrag",
    "de.agent.decision.made",
    "de.billing.rechnung.erstellt",
    "de.billing.xrechnung.b2g.ready",
    "de.eeg.anlage.foerderung-auslaufend",
    "de.eeg.anlage.mastr-registriert",
    "de.eeg.marktpraemie.berechnet",
    "de.eeg.settlement.batch-due",
    "de.eeg.veraeusserungsform.gewechselt",
    "de.eeg.verguetung.berechnet",
    "de.gabi.alocat.missing",
    "de.gabi.nomination.curtailed",
    "de.gabi.nomination.rejected",
    "de.gabi.nomres.missing",
    "de.invoic.payment.overdue",
    "de.invoic.receipt.disputed",
    "de.invoic.receipt.settled",
    "de.mabis.korrekturbedarf.opened",
    "de.mabis.submission.failed",
    "de.mako.abmeldeanfrage.beantwortet",
    "de.mako.antwortfrist.abgelaufen",
    "de.mako.aperak.accepted",
    "de.mako.aperak.rejected",
    "de.mako.contrl.received",
    "de.mako.dead-letter.recorded",
    "de.mako.edifact.outbound",
    "de.mako.malo.identified",
    "de.mako.netzzugang.uebermittlungsbedarf",
    "de.mako.process.completed",
    "de.mako.process.failed",
    "de.mako.process.initiated",
    "de.markt.einwilligung.erteilt",
    "de.markt.einwilligung.widerrufen",
    "de.markt.geraet.konfiguration.updated",
    "de.markt.malo.stammdaten-geaendert",
    "de.markt.malo.updated",
    "de.markt.melo.updated",
    "de.markt.mmma.import.failed",
    "de.markt.mmma.import.success",
    "de.markt.msb-rahmenvertrag.updated",
    "de.markt.nb-contract.updated",
    "de.markt.netzzugang.antrag.updated",
    "de.markt.partner.updated",
    "de.markt.pricat.published",
    "de.markt.sr.konfigurationsprodukt.updated",
    "de.markt.stammdaten.geaendert",
    "de.markt.versorgung.changed",
    "de.markt.versorgung.eog-begonnen",
    "de.markt.versorgung.ersatz-auslaufend",
    "de.markt.versorgung.gap-detected",
    "de.messwert.cls.compliance-issue",
    "de.messwert.cls.compliance-resolved",
    "de.messwert.esa.typ2.delivery.overdue",
    "de.messwert.esa.typ2.delivery.resumed",
    "de.messwert.reading.confirmation.overdue",
    "de.messwert.reading.delivery.overdue",
    "de.messwert.reading.delivery.resumed",
    "de.messwert.reading.direct.stored",
    "de.messwert.reading.order.failed",
    "de.messwert.reading.quality.warning",
    "de.messwert.smgw.cert.expiry-warning",
    "de.netzbilanz.invoic.dispatch-overdue",
    "de.netzbilanz.invoic.dispatched",
    "de.netzbilanz.invoic.disputed",
    "de.netzbilanz.invoic.drafted",
    "de.netzbilanz.invoic.paid",
    "de.netzbilanz.kostenblatt.computed",
    "de.obs.deadline.approaching",
    "de.obs.stp.parity.alert",
    "de.sperr.auftrag.eingegangen",
    "de.sperr.ausfuehrung.ueberfaellig",
    "de.sperr.ausgefuehrt",
    "de.sperr.fehlgeschlagen",
    "de.sperr.iftsta.ausstehend",
    "de.sperr.storniert",
    "de.sperr.versuch.gescheitert",
    "de.tarif.angebot.angenommen",
    "de.tarif.product.updated",
    "de.vertrag.abgeschlossen",
    "de.vertrag.ablauf.ankuendigung",
    "de.vertrag.aktiv",
    "de.vertrag.autoerneuerung.ankuendigung",
    "de.vertrag.bestellung.abgelehnt",
    "de.vertrag.bestellung.angenommen",
    "de.vertrag.bestellung.eingegangen",
    "de.vertrag.gekuendigt",
    "de.vertrag.kuendigung",
    "de.vertrag.kuendigung-widerrufen",
    "de.vertrag.preisaenderung.ankuendigung",
    "de.vertrag.preisgarantie-hinterlegt",
    "de.vertrag.tarifwechsel",
    "de.vertrag.tarifwechsel-geplant",
    "de.vertrag.widerruf.eingegangen",
    "de.vpp.dispatch.confirmed",
    "de.vpp.settlement.berechnet",
];

/// One catalog constant.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    module: String,
    name: String,
    value: String,
    phantom: bool,
    summary: String,
}

/// The catalog constants of `src`, in declaration order.
fn parse_catalog(src: &str) -> Vec<Entry> {
    let catalog = src.split("#[cfg(test)]").next().unwrap_or_default();
    let lines: Vec<&str> = catalog.lines().collect();
    let mut module = String::new();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("pub mod ") {
            module = rest.trim_end_matches(" {").trim().to_owned();
            continue;
        }
        let Some(rest) = t.strip_prefix("pub const ") else {
            continue;
        };
        let Some(name) = rest.split(':').next() else {
            continue;
        };
        let Some(value) = rest
            .split_once("= \"")
            .and_then(|(_, v)| v.split('"').next())
        else {
            continue;
        };
        let mut doc = Vec::new();
        for prev in lines[..i].iter().rev() {
            let p = prev.trim();
            if let Some(d) = p.strip_prefix("///") {
                doc.push(d.trim().to_owned());
            } else if !p.is_empty() {
                break;
            }
        }
        doc.reverse();
        let phantom = doc.iter().any(|d| d.contains("⚠ phantom:"));
        let summary = without_intra_doc_links(
            &doc.iter()
                .take_while(|d| !d.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
        );
        out.push(Entry {
            module: module.clone(),
            name: name.trim().to_owned(),
            value: value.to_owned(),
            phantom,
            summary,
        });
    }
    out
}

/// A doc comment's rustdoc intra-doc links (`` [`X`] `` with no target) as plain
/// code spans: Markdown has no intra-doc links, so the page would show the
/// brackets. A real link (`` [`X`](url) ``) is kept.
fn without_intra_doc_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[`") {
        let after = &rest[at + 2..];
        let Some(close) = after.find("`]") else { break };
        let tail = &after[close + 2..];
        out.push_str(&rest[..at]);
        if tail.starts_with('(') || tail.starts_with('[') {
            out.push_str(&rest[at..at + 2 + close + 2]);
        } else {
            out.push('`');
            out.push_str(&after[..close]);
            out.push('`');
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// `events/<type>.json` of every crate under `crates/` and `services/`:
/// type → (crate, file).
fn schema_sources(root: &Path) -> Result<BTreeMap<String, (String, PathBuf)>, Vec<String>> {
    let mut out: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    let mut problems = Vec::new();
    for tree in ["crates", "services"] {
        let Ok(dirs) = std::fs::read_dir(root.join(tree)) else {
            continue;
        };
        let mut dirs: Vec<_> = dirs.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(files) = std::fs::read_dir(dir.join("events")) else {
                continue;
            };
            let krate = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let mut files: Vec<_> = files.flatten().map(|e| e.path()).collect();
            files.sort();
            for file in files {
                let Some(ty) = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".json"))
                else {
                    continue;
                };
                if let Some((other, _)) = out.get(ty) {
                    problems.push(format!(
                        "{ty} has a schema in both {other} and {krate} — one crate emits a type"
                    ));
                    continue;
                }
                out.insert(ty.to_owned(), (krate.clone(), file.clone()));
            }
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// The services naming each constant (`module::NAME`) in production source.
fn references(root: &Path, entries: &[Entry]) -> BTreeMap<String, BTreeSet<String>> {
    let mut texts: Vec<(String, String)> = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root.join("services")) else {
        return BTreeMap::new();
    };
    for dir in dirs.flatten() {
        let svc = dir.file_name().to_string_lossy().into_owned();
        let mut files = Vec::new();
        crate::check_workflow_purity::walk_rs(&dir.path().join("src"), &mut files);
        let mut text = String::new();
        for f in files {
            if let Ok(src) = std::fs::read_to_string(&f) {
                text.push_str(&crate::check_error_tuples::production_text(&src));
                text.push('\n');
            }
        }
        texts.push((svc, text));
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in entries {
        let needle = format!("{}::{}", e.module, e.name);
        for (svc, text) in &texts {
            let named = text.match_indices(&needle).any(|(at, _)| {
                text[at + needle.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            });
            if named {
                out.entry(e.value.clone()).or_default().insert(svc.clone());
            }
        }
    }
    out
}

/// What the ratchet says, for `entries` with schemas for `typed`.
fn ratchet(entries: &[Entry], typed: &BTreeSet<String>, untyped: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let listed: BTreeSet<&str> = untyped.iter().copied().collect();
    for e in entries {
        let has_schema = typed.contains(&e.value);
        let is_listed = listed.contains(e.value.as_str());
        if !e.phantom && !has_schema && !is_listed {
            out.push(format!(
                "{} is emitted with no payload bound to it — emit it through \
                 `CloudEvent::typed` with a `mako_service::EventPayload` and commit its \
                 `events/{}.json`",
                e.value, e.value
            ));
        }
        if is_listed && has_schema {
            out.push(format!(
                "{} has a schema now — remove it from UNTYPED in xtask/src/event_catalog.rs",
                e.value
            ));
        }
        if is_listed && e.phantom {
            out.push(format!(
                "{} is a phantom — nothing emits it; remove it from UNTYPED",
                e.value
            ));
        }
    }
    for ty in untyped {
        if !entries.iter().any(|e| e.value == *ty) {
            out.push(format!(
                "{ty} is in UNTYPED and not in the catalog — remove its entry"
            ));
        }
    }
    for ty in typed {
        if !entries.iter().any(|e| e.value == *ty) {
            out.push(format!(
                "events/{ty}.json names a type the catalog does not declare"
            ));
        }
    }
    out
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// The page text.
fn render(
    entries: &[Entry],
    schemas: &BTreeMap<String, (String, PathBuf)>,
    refs: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let typed = entries
        .iter()
        .filter(|e| schemas.contains_key(&e.value))
        .count();
    let phantoms = entries.iter().filter(|e| e.phantom).count();
    let mut out = String::from(
        "+++\n\
         title = \"Event catalog\"\n\
         description = \"Every CloudEvent type mako declares: who emits it, who names it, and the JSON Schema of its data.\"\n\
         weight = 20\n\
         +++\n\
         <!-- Generated by `cargo xtask event-catalog` from mako-events, the committed\n     \
         events/*.json and the services' source. Do not edit: `cargo xtask\n     \
         event-catalog --check` refuses a page that differs. -->\n\n",
    );
    let _ = write!(
        out,
        "Every type an integrator can subscribe to, with a webhook endpoint or the\n\
         `GET /api/v1/events` feed of the service that emits it. A type with a schema\n\
         is emitted through a payload bound to it, and its events carry the schema's\n\
         URL in `dataschema`. A **phantom** is declared and emitted by nothing.\n\n\
         {} types: {typed} with a schema, {phantoms} phantoms.\n\n\
         | Type | Emitted by | Named in | Schema | What it says |\n\
         |---|---|---|---|---|\n",
        entries.len()
    );
    for e in entries {
        let emitter = schemas.get(&e.value).map(|(k, _)| k.as_str());
        let named = refs
            .get(&e.value)
            .map(|s| s.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let schema = if emitter.is_some() {
            format!("[schema]({BASE}{}.json)", e.value)
        } else {
            "—".to_owned()
        };
        let what = if e.phantom {
            format!("**phantom** — {}", cell(&e.summary))
        } else {
            cell(&e.summary)
        };
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {schema} | {what} |",
            e.value,
            emitter.unwrap_or("—"),
            if named.is_empty() {
                "—".to_owned()
            } else {
                named
            },
        );
    }
    out
}

/// Run the generator; `check` compares instead of writing.
pub fn run(root: &Path, check: bool) -> bool {
    let Ok(src) = std::fs::read_to_string(root.join(CATALOG)) else {
        eprintln!("event-catalog: cannot read {CATALOG}");
        return false;
    };
    let entries = parse_catalog(&src);
    if entries.len() < 80 {
        eprintln!(
            "event-catalog: parsed {} types from {CATALOG}; the parser broke, not the catalog",
            entries.len()
        );
        return false;
    }
    let schemas = match schema_sources(root) {
        Ok(s) => s,
        Err(problems) => {
            for p in problems {
                eprintln!("event-catalog: {p}");
            }
            return false;
        }
    };
    let typed: BTreeSet<String> = schemas.keys().cloned().collect();
    let mut problems = ratchet(&entries, &typed, UNTYPED);
    let refs = references(root, &entries);
    let page = render(&entries, &schemas, &refs);

    let mut published: BTreeMap<String, String> = BTreeMap::new();
    for (ty, (_, file)) in &schemas {
        match std::fs::read_to_string(file) {
            Ok(text) => {
                published.insert(format!("{ty}.json"), text);
            }
            Err(e) => problems.push(format!("{}: {e}", file.display())),
        }
    }
    let dir = root.join(SCHEMAS);
    if check {
        if std::fs::read_to_string(root.join(PAGE)).ok().as_deref() != Some(page.as_str()) {
            problems.push(format!("{PAGE} is stale — run `cargo xtask event-catalog`"));
        }
        let mut on_disk: BTreeMap<String, String> = BTreeMap::new();
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                if let Ok(text) = std::fs::read_to_string(f.path()) {
                    on_disk.insert(f.file_name().to_string_lossy().into_owned(), text);
                }
            }
        }
        if on_disk != published {
            problems.push(format!(
                "{SCHEMAS}/ differs from the committed events/*.json — run `cargo xtask event-catalog`"
            ));
        }
    } else {
        let write = || -> std::io::Result<()> {
            std::fs::write(root.join(PAGE), &page)?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            std::fs::create_dir_all(&dir)?;
            for (name, text) in &published {
                std::fs::write(dir.join(name), text)?;
            }
            Ok(())
        };
        if let Err(e) = write() {
            problems.push(format!("writing the catalog: {e}"));
        }
    }
    if problems.is_empty() {
        println!(
            "event-catalog: {} types, {} with a schema, {} untyped in the ratchet",
            entries.len(),
            typed.len(),
            UNTYPED.len()
        );
        return true;
    }
    for p in &problems {
        eprintln!("event-catalog: {p}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_intra_doc_link_becomes_a_code_span_and_a_real_link_stays() {
        assert_eq!(
            without_intra_doc_links(
                "emits [`GEKUENDIGT`] per child, see [`docs`](https://x.example)"
            ),
            "emits `GEKUENDIGT` per child, see [`docs`](https://x.example)"
        );
    }

    const SRC: &str = r#"
pub mod vertrag {
    /// Ein Vertrag wurde gekündigt.
    pub const GEKUENDIGT: &str = "de.vertrag.gekuendigt";
    /// Not emitted.
    ///
    /// ⚠ phantom: no emitter.
    pub const GEISTER: &str = "de.vertrag.geister";
    /// Emitted, bound.
    pub const AKTIV: &str = "de.vertrag.aktiv";
}
#[cfg(test)]
mod tests {
}
"#;

    #[test]
    fn the_catalog_is_parsed_with_its_phantoms() {
        let e = parse_catalog(SRC);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].value, "de.vertrag.gekuendigt");
        assert_eq!(e[0].summary, "Ein Vertrag wurde gekündigt.");
        assert!(!e[0].phantom && e[1].phantom);
        assert_eq!(e[1].summary, "Not emitted.");
    }

    #[test]
    fn a_type_leaving_the_ratchet_needs_a_schema() {
        let e = parse_catalog(SRC);
        let typed = BTreeSet::from(["de.vertrag.aktiv".to_owned()]);
        assert!(ratchet(&e, &typed, &["de.vertrag.gekuendigt"]).is_empty());
        let left = ratchet(&e, &typed, &[]);
        assert_eq!(left.len(), 1);
        assert!(
            left[0].starts_with("de.vertrag.gekuendigt is emitted with no payload"),
            "{left:?}"
        );
        let stale = ratchet(&e, &typed, &["de.vertrag.gekuendigt", "de.vertrag.aktiv"]);
        assert!(
            stale.iter().any(|p| p.contains("has a schema now")),
            "{stale:?}"
        );
        let phantom = ratchet(&e, &typed, &["de.vertrag.gekuendigt", "de.vertrag.geister"]);
        assert!(
            phantom.iter().any(|p| p.contains("is a phantom")),
            "{phantom:?}"
        );
    }

    /// A fixture tree with the real catalog and the real schemas: generated,
    /// it checks clean; with one row edited, `--check` refuses it.
    #[test]
    fn a_stale_page_fails() {
        let real = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let root = std::env::temp_dir().join(format!("event-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/mako-events/src")).unwrap();
        std::fs::create_dir_all(root.join("site/content/docs/reference")).unwrap();
        std::fs::create_dir_all(root.join("services")).unwrap();
        std::fs::copy(real.join(CATALOG), root.join(CATALOG)).unwrap();
        for (ty, (krate, file)) in schema_sources(real).unwrap() {
            let dir = root.join("crates").join(krate).join("events");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::copy(file, dir.join(format!("{ty}.json"))).unwrap();
        }
        assert!(run(&root, false), "the fixture generates");
        assert!(run(&root, true), "a generated page checks clean");
        let page = std::fs::read_to_string(root.join(PAGE)).unwrap();
        std::fs::write(root.join(PAGE), page.replacen("| `de.", "| `xx.", 1)).unwrap();
        let stale = run(&root, true);
        std::fs::write(root.join(PAGE), &page).unwrap();
        std::fs::remove_dir_all(root.join(SCHEMAS)).unwrap();
        let missing_schemas = run(&root, true);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!stale, "a stale page passed --check");
        assert!(!missing_schemas, "unpublished schemas passed --check");
    }

    /// The committed page, schemas and ratchet are current.
    #[test]
    fn the_real_tree_is_current() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        assert!(run(root, true));
    }

    /// The page links the schemas where `mako_service` says they are served.
    #[test]
    fn the_base_is_mako_services() {
        let src = std::fs::read_to_string(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
                .join("crates/mako-service/src/cloudevent.rs"),
        )
        .unwrap();
        assert!(src.contains(&format!("pub const SCHEMA_BASE: &str = \"{BASE}\";")));
    }
}
