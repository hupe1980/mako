//! Guard: no claim the root `AGENTS.md` lists as known-wrong stands in tracked
//! text, and no tracked file points into the untracked planning trees.
//!
//! The *Known-wrong legal claims* table in `AGENTS.md` records statutory and
//! regulatory near-misses that shipped once. A table is a list a reader has to
//! remember; this guard is the check that reads it. A plausible wrong `§` or
//! Frist is indistinguishable from a right one to anyone not holding the
//! document, so the only defence is to refuse the phrase itself.
//!
//! ## Rule 1 — the phrase list
//!
//! [`RULES`] holds one regular expression per table row a phrase can catch,
//! each naming its row by the row's first cell. The expression runs against
//! one line at a time after [`normalise`] strips Markdown emphasis and code
//! ticks and spaces every `§` from its number, so `**§14 GasNEV**` and
//! `§ 14 GasNEV` are one spelling. A claim split across two lines is not
//! caught; the patterns are written for the spellings the tree has carried.
//!
//! A sentence that quotes a wrong claim in order to refute it is admitted by an
//! entry in [`ALLOWED`]: the file, the rule, a fragment of the admitted line and
//! the reason. The fragment pins the entry to that sentence rather than to the
//! whole file, so a second, unrefuted occurrence in the same file still fails.
//!
//! **Scope.** The root `*.md` files and every tracked or not-ignored file under
//! `crates/`, `services/`, `site/`, `demos/` and `makotest/` with a text extension (`.rs`, `.md`,
//! `.sql`, `.cedar`, `.toml`, `.yaml`, `.yml`, `.sh`, `.py`, `.html`, `.txt`).
//! `.json` is excluded: the JSON carrying regulatory text is
//! `crates/edi-energy/profiles/**`, BDEW's own AHB extracted verbatim. The
//! root `AGENTS.md` table states every wrong claim in its first column, so rule
//! 1 skips the table's lines and reads the rest of the file.
//!
//! **Rows no phrase catches.** These are design claims rather than phrases, or
//! claims whose wrong and right forms share every word; they are held by the
//! tests and guards named in their rows, not by this one: see [`UNCOVERED`].
//! A table row in neither list fails the guard, so a new row forces the
//! decision.
//!
//! ## Rule 2 — pointers into the untracked planning trees
//!
//! The roadmap and the feature specifications live in `concepts/` and `specs/`,
//! both gitignored. A tracked file that names `ROADMAP.md`, `concepts/` or
//! `specs/` points a reader at text the checkout does not have. Refused in
//! every tracked or not-ignored file, except [`POINTER_SITES`]: `.gitignore`
//! and the code that opens those paths, each with its reason.

use std::path::Path;
use std::process::Command;

use regex::Regex;

/// One known-wrong claim a phrase can catch.
struct Rule {
    /// Short identifier, used by [`ALLOWED`].
    id: &'static str,
    /// The `AGENTS.md` table row this enforces — a substring of its first cell.
    row: &'static str,
    /// Regular expression run against one [`normalise`]d line.
    pattern: &'static str,
    /// What holds instead.
    reason: &'static str,
}

/// The phrase list. Each `row` must occur in the `AGENTS.md` table.
const RULES: &[Rule] = &[
    Rule {
        id: "aperak-weekday",
        row: "The Strom UTILMD/ORDERS APERAK window is 45 Minuten",
        pattern: r"(?i)45[ -]?(min\b|minuten|minutes|-minute).{0,40}(weekday|mo[–-]fr|montag|on a werktag|am werktag|an werktagen|werktags)|(weekday|mo[–-]fr\b|montag bis freitag).{0,40}45[ -]?(min\b|minuten|minutes)",
        reason: "APERAK AHB 1.1 § 2.4.1 states the 45 Minuten unqualified and carves out \
                 Samstag alone — no weekday or Werktag qualifier; render it from \
                 `mako_fristen::aperak_strom_due_at`",
    },
    Rule {
        id: "aperak-gas-initial-noon",
        row: "The Gas APERAK Initialprozess window is 3 Werktage",
        pattern: r"(?i)\b3\.? ?(werktage?n?|wt)\b[^.;|()]{0,12}12 ?(uhr|:00)",
        reason: "APERAK AHB 1.1 § 2.3.1: „spätestens 3 Werktage nach Eingang\" names no clock \
                 time; the 12 Uhr belongs to the Folgeprozess sentence",
    },
    Rule {
        id: "seventeen-cutoff",
        row: "A Werktage-Antwortfrist expires at a 17:00",
        pattern: r"(?i)17(:00| uhr).{0,40}(cut-?off|end-of-business|geschäftsschluss)|(cut-?off|end-of-business|geschäftsschluss).{0,40}17(:00| uhr)|17:00 europe[/-]berlin",
        reason: "no BDEW or BNetzA document in this domain states a 17:00 cut-off; a Frist \
                 naming a Werktag runs to the end of that day",
    },
    Rule {
        id: "gpke-kap-1-7",
        row: "GPKE Teil 1 Kap. 1.7 defines the Werktag",
        pattern: r"GPKE Teil 1,? Kap(\.|itel)? ?1\.7",
        reason: "GPKE Teil 1 defines the Werktag in Kap. 2 and applies it in Kap. 7 \
                 „Fristenberechnung\"; Kap. 1.7 is the WiM Teil 1 (Anlage 2a) chapter that \
                 defers to it. Narrower than the row: only the spelling „GPKE Teil 1 \
                 Kap. 1.7\" is caught, not the chapter named before the document",
    },
    Rule {
        id: "asx-sign-only",
        row: "`asx-rs` accepts a sign-only override",
        pattern: r"(?i)asx-rs.{0,30}refuses (a|the) (policy )?layer only when|sign-only (override|overlay).{0,30}(passes|is valid|validates)",
        reason: "asx-rs refuses a sign-only overlay itself: the declared security_floor is \
                 SIGN_AND_ENCRYPT, so dropping encryption fails as SecurityFloorViolation",
    },
    Rule {
        id: "messzv",
        row: "Any \"§… MessZV\" citation",
        pattern: r"MessZV|Messzugangsverordnung",
        reason: "the MessZV was repealed by Art. 12 G. v. 29.08.2016 and folded into the MsbG",
    },
    Rule {
        id: "eeg-20-absatz",
        row: "§20 Abs. 2 + Anlage 1 EEG 2023",
        pattern: r"§ 20 Abs\.[^;|/]{0,30}EEG 2023",
        reason: "§ 20 EEG 2023 (Marktprämie) has Sätze, no Absätze; Anlage 1 Nr. 2 decides \
                 when the Jahresmarktwert applies and Nr. 4 computes it (Nr. 4.3.1 the \
                 energieträgerspezifischen categories). Narrower than the row: an Absatz \
                 is caught only where „EEG 2023\" follows, since earlier EEG editions are \
                 not this row's claim",
    },
    Rule {
        id: "enwg-6b-integrated",
        row: "§6b EnWG = an integrated NB+LF deployment",
        pattern: r"(?i)§ 6b EnWG.{0,40}(deploy|integrat|integriert)|(deploy|integrat|integriert).{0,40}§ 6b EnWG",
        reason: "§ 6b EnWG is Rechnungslegung und Buchführung — the accounts a vertikal \
                 integriertes Unternehmen keeps — not what makes a deployment integrated",
    },
    Rule {
        id: "stromnev-18-blind",
        row: "Blindmehrarbeit rests on StromNEV §18",
        pattern: r"(?i)§ 18 StromNEV.{0,60}blind|blind\w*.{0,60}§ 18 StromNEV",
        reason: "§ 18 StromNEV is the Entgelt für dezentrale Erzeugung; reactive energy is \
                 charged from the Preisblatt formed under § 17 StromNEV. Narrower than the \
                 row: the free-share constant (tan φ) it names is a design claim, not \
                 matched",
    },
    Rule {
        id: "gasnev-14",
        row: "Gas NNE Grundpreis / Arbeitspreis rests on §14 GasNEV",
        pattern: r"§ 14 GasNEV",
        reason: "§ 14 GasNEV is Teilnetze; Netzentgelte are § 15 GasNEV, the Verrechnungspreis \
                 § 15 Abs. 7",
    },
    Rule {
        id: "eeg-42b",
        row: "Gemeinschaftliche Gebäudeversorgung is §42b",
        pattern: r"§ 42b EEG",
        reason: "there is no § 42b EEG 2023; GGV is § 42b EnWG",
    },
    Rule {
        id: "enwg-41-3",
        row: "§41 Abs. 3 EnWG = 6-week price-change notice",
        pattern: r"(?i)§ 41 Abs\. 3 EnWG.{0,60}(woche|week|preisänderung|price)|(woche|week|preisänderung|price).{0,60}§ 41 Abs\. 3 EnWG",
        reason: "§ 41 Abs. 3 EnWG is an advertising-information duty; six weeks is § 5 Abs. 2 \
                 StromGVV/GasGVV, Sonderverträge § 41 Abs. 5 EnWG",
    },
    Rule {
        id: "eeg-38a-mieterstrom",
        row: "§38a EEG 2023 = Mieterstromzuschlag",
        pattern: r"(?i)§ 38a EEG.{0,60}mieterstrom|mieterstrom\w*.{0,60}§ 38a EEG",
        reason: "§ 38a EEG 2023 is Zahlungsberechtigungen; the Mieterstromzuschlag is § 21 \
                 Abs. 3 EEG 2023",
    },
    Rule {
        id: "msbg-29-100kw",
        row: "§29 MsbG mandatory iMSys 7–100 kW band",
        pattern: r"(?i)§ 29 MsbG.{0,80}100 ?kW|\b7 ?[–-] ?100 ?kW",
        reason: "§ 29 Abs. 1 Nr. 2b MsbG has only a > 7 kW lower bound; no 100 kW cap exists",
    },
    Rule {
        id: "enwg-40a-abschlag",
        row: "§40a EnWG = Abschlagszahlungen",
        pattern: r"(?i)§ 40a( Abs\. \d)? EnWG[^·;,|]{0,30}abschlag|abschlag\w*\s*\(§ 40a",
        reason: "§ 40a EnWG is Verbrauchsermittlung; Abschläge are § 13 StromGVV/GasGVV. \
                 Narrower than the row: its § 40c deadline half is not matched",
    },
    Rule {
        id: "enwg-41b-dynamic",
        row: "dynamic-tariff iMSys requirement = §41b EnWG",
        pattern: r"(?i)§ 41b EnWG.{0,40}(dynamisch|dynamic|imsys)|(dynamisch|dynamic)\w*.{0,40}§ 41b EnWG",
        reason: "§ 41b EnWG is Haushaltskunden-Lieferverträge außerhalb der Grundversorgung; \
                 dynamic tariffs are § 41a EnWG",
    },
    Rule {
        id: "eeg-53b-regional",
        row: "§53b EEG = regional Grünstromkennzeichnung",
        pattern: r"(?i)§ 53b.{0,60}(grünstrom|netzgebiet|grid[- ]area|configurable)",
        reason: "§ 53b EEG 2023 is Regionalnachweise: a fixed 0,1 ct/kWh cut to a gesetzlich \
                 bestimmter anzulegender Wert",
    },
    Rule {
        id: "eeg-53c-oversupply",
        row: "§53c EEG = structural-oversupply reduction",
        pattern: r"(?i)§ 53c.{0,60}(oversupply|überangebot|not yet operational|energiestg)",
        reason: "§ 53c EEG 2023 is the Stromsteuerbefreiung reduction under the StromStG, and \
                 is operative law",
    },
    Rule {
        id: "eeg-54-generic",
        row: "§54 EEG = generic BNetzA Ausschreibungsreduzierung",
        pattern: r"(?i)§ 54 EEG.{0,80}(§ 36d|§ 37a)",
        reason: "§ 54 EEG 2023 is Ausschreibungen für Solaranlagen des ersten Segments only. \
                 Narrower than the row: caught only beside the two neighbours the row names \
                 (§ 36d, § 37a); a generic-reduction reading in other words passes",
    },
    Rule {
        id: "eeg-35a-erloeschen",
        row: "Zuschlag-Erlöschen = §35a EEG",
        pattern: r"(?i)§ (35a|33|55)\b( EEG)?.{0,30}(erlösch|erlisch|expir|lapse)|(erlösch|erlisch|expir|lapse)\w*.{0,30}§ (35a|33|55)\b",
        reason: "expiry for want of commissioning is §§ 36e / 37e / 39e EEG 2023; § 35a is \
                 Entwertung von Zuschlägen, § 33 Ausschluss von Geboten, § 55 Pönalen. \
                 Narrower than the row: a § 33 or § 55 is caught only within 30 characters \
                 of an expiry word, so the bare citation of either for something else passes",
    },
    Rule {
        id: "modul-2-3",
        row: "§14a EnWG Modul 2 = zeitvariable/HT-NT Netzentgelte",
        pattern: r"(?i)modul ?2\b[^,;.]{0,25}(zeitvariabl|ht/nt|ht-nt|\btou\b)|modul ?3\b.{0,20}steuerungsentschädigung|BK6-22-300.{0,10}§ ?14a Modul",
        reason: "BK8-22/010-A numbers the modules: Modul 2 is the prozentuale \
                 Arbeitspreis-Reduzierung (40 %), Modul 3 the zeitvariable Netzentgelt. The \
                 row's „Modul 3 = Spotpreis\" half is not matched",
    },
    Rule {
        id: "invoic-31009-direction",
        row: "INVOIC 31009 (MSB-Rechnung) is NB → MSB",
        pattern: r"31009.{0,60}NB ?(→|->) ?MSB",
        reason: "31009 is MSB → NB / LF / ESA; the MSB is the invoicer. Narrower than the \
                 row: only the arrow spelling „NB → MSB\" after the PID is caught; a \
                 modelled `nb_mp_id` recipient is a design defect no phrase shows",
    },
    Rule {
        id: "pid-13025",
        row: "13025 is not a real PID",
        pattern: r"(?i)13025.{0,40}(is not a|does not exist|nicht vergeben|unassigned)",
        reason: "13025 is published (Lastgang Marktlokation, Tranche); 13024 is the absent one",
    },
    Rule {
        id: "ust-12-3-pv",
        row: "§12 Abs. 3 UStG 0 % applies to PV electricity",
        pattern: r"(?i)§ 12 Abs\. 3 UStG.{0,40}(30 ?kWp|feed-in|einspeis)|kWp[^.;|]{0,60}§ 19\b|§ 19 (UStG )?(limit|grenze)\b",
        reason: "§ 12 Abs. 3 UStG zero-rates the supply of the PV installation, never \
                 electricity or a feed-in Gutschrift; the Kleinunternehmerregelung (§ 19 \
                 UStG) is an election bounded by turnover, never by plant size. Narrower \
                 than the row: the § 19 half is caught beside a kWp figure or as a bare \
                 „§ 19 limit/Grenze\"; a „§ 19 turnover/Kleinunternehmer limit\" names the \
                 real Umsatzgrenze and passes",
    },
    Rule {
        id: "fv2027-04-release",
        row: "API-Webdienste 2.0.0 applies from 01.10.2027",
        pattern: r"(?i)RELEASE_2_0_0|Nr\. 57 proposes|API-Webdienste[^.|]{0,40}2\.0\.0[^.|]{0,60}01\.10\.2027|2\.0\.0[^.|]{0,40}ab 01\.10\.2027|UTILTS[^.|]{0,60}(ersetzt|retire|abgelöst)|(ersetzt|retire[sd]?|abgelöst)[^.|]{0,30}UTILTS",
        reason: "BNetzA Mitteilung Nr. 58 (01.10.2026) publishes no EDIFACT and no \
                 API-Webdienste document for 01.04.2027 — only ActivationDocument AWT 1.1g and \
                 the Regelungen zum Übertragungsweg 1.11 bind then — and keeps UTILTS",
    },
    Rule {
        id: "aperak-ahb-1-0",
        row: "APERAK AHB 1.0 states the current APERAK rule",
        pattern: r"APERAK AHB 1\.0",
        reason: "APERAK AHB 1.1 binds from FV2026-10-01 (profiles/sources.json); cite it, or \
                 admit an edition-specific sentence in ALLOWED",
    },
    Rule {
        id: "role-features-6a",
        row: "Compile-time role features enforce § 6a EnWG",
        pattern: r"(?i)(compile[- ]time|cargo[- ]features?|role[- ](builds?|features?|scoped|gated)|build[- ]flags?|binary sha).{0,80}§ 6a|§ 6a.{0,60}(compile[- ]time|cargo[- ]features?|role[- ](builds?|features?|scoped|gated)|build[- ]flags?)",
        reason: "a role build cannot answer as a role it was not built for — that fails a \
                 misconfiguration closed and discharges no unbundling duty; the \
                 confidentiality of § 6a Abs. 1 EnWG is the deployment's access policy (Cedar)",
    },
];

/// Table rows no phrase can catch, by the first cell's text.
const UNCOVERED: &[&str] = &[
    // The refutation and the claim share every word („the 2022 cut never
    // reached heating gas"); the energy-billing rate table holds the rule.
    "2022 heating-gas Energiesteuer = 0",
    "MaBiS vorläufig day 3 / endgültig day 8",
    "§42c Energy Sharing reduces Netzentgelte",
    "invoice content (Kilowattstundenpreis",
    "§24 EEG Anlagenzusammenfassung requires operator identity",
    "A §-based reduction can be subtracted from the settled euro amount",
    "A `None` from a period-rate helper can fall back to a default rate",
    "§40c EnWG's three-week deadline follows from a short billing period",
    "the Modul-2 percentage is the Netzbetreiber's to publish",
    "The ESA Werteanfrage shares REQOTE 35002",
    // A Frist stated beside a source is `check-frist-literals`'s.
    "WiM MSB-Wechsel responses are 5 Werktage",
    "A MIG defines where a data element sits in a segment",
    "Parse-don't-validate applies uniformly",
    "A DB `CHECK` is enough",
    "§13a Abs. 2 EnWG compensation uses one Ausfallarbeit basis",
];

/// A sentence admitted although it matches a rule.
struct Allow {
    /// Workspace-relative path.
    path: &'static str,
    /// The [`Rule::id`] it is admitted under.
    rule: &'static str,
    /// A fragment of the admitted (normalised) line.
    fragment: &'static str,
    /// Why the sentence is right. Read by the tests; for the guard it is the
    /// entry's documentation.
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
    reason: &'static str,
}

/// Sentences that quote a known-wrong claim to refute it, or name a superseded
/// edition for what that edition itself states.
const ALLOWED: &[Allow] = &[
    Allow {
        path: "AGENTS.md",
        rule: "aperak-weekday",
        fragment: "the 45 minutes carries no Montag–Freitag qualifier",
        reason: "states that the qualifier does not exist",
    },
    Allow {
        path: "crates/mako-fristen/src/lib.rs",
        rule: "seventeen-cutoff",
        fragment: "Sizing such a Frist at 17:00 expires it seven hours",
        reason: "states why a day-granular Frist does not end at 17:00",
    },
    Allow {
        path: "makotest/tests/test_deadlines.py",
        rule: "seventeen-cutoff",
        fragment: "Sizing it at 17:00 expires it seven hours early",
        reason: "refutes the cut-off",
    },
    Allow {
        path: "crates/energy-billing/tests/it/calculator_tests.rs",
        rule: "ust-12-3-pv",
        fragment: "zero-rates the PV system, not the feed-in",
        reason: "states what § 12 Abs. 3 UStG does and does not reach",
    },
    Allow {
        path: "crates/mako-fristen/src/lib.rs",
        rule: "seventeen-cutoff",
        fragment: "the second was placed at a 17:00",
        reason: "the test doc states that no document names the cut-off",
    },
    Allow {
        path: "makotest/tests/test_deadlines.py",
        rule: "seventeen-cutoff",
        fragment: "placing WiM at a 17:00 cut-off expires it seven hours early",
        reason: "refutes the cut-off",
    },
    Allow {
        path: "makotest/tests/test_deadlines.py",
        rule: "seventeen-cutoff",
        fragment: "End of the Werktag, not a 17:00 cut-off",
        reason: "refutes the cut-off",
    },
    Allow {
        path: "crates/edi-energy/src/builders/aperak.rs",
        rule: "aperak-ahb-1-0",
        fragment: "APERAK AHB 1.0 makes Nr 00017",
        reason: "states what the FV2025-10-01 profile (AHB 1.0) requires of a message built \
                 under it",
    },
    Allow {
        path: "crates/mako-engine/src/erc.rs",
        rule: "aperak-ahb-1-0",
        fragment: "APERAK AHB 1.0 / 1.1",
        reason: "names both editions the active profiles carry",
    },
    Allow {
        path: "crates/mako-engine/src/erc.rs",
        rule: "aperak-ahb-1-0",
        fragment: "APERAK AHB 1.0 admits 27 for 29001",
        reason: "states what the FV2025-10-01 profile admits",
    },
    Allow {
        path: "services/makod/tests/it/e2e_aperak_conformance.rs",
        rule: "aperak-ahb-1-0",
        fragment: "APERAK AHB 1.0 admits",
        reason: "states what the FV2025-10-01 profile the test builds under admits",
    },
];

/// Files that may name `ROADMAP.md`, `concepts/` or `specs/`, and why.
const POINTER_SITES: &[(&str, &str)] = &[
    (".gitignore", "it is what makes the trees untracked"),
    (
        "xtask/src/check_known_wrong.rs",
        "this guard names what it refuses",
    ),
    (
        "xtask/src/check_specs.rs",
        "opens specs/ and the roadmap to compare them",
    ),
    (
        "xtask/src/check_citations.rs",
        "scans concepts/ and specs/ when present",
    ),
    (
        "xtask/src/check_dep_versions.rs",
        "scans concepts/ and specs/ when present",
    ),
    (
        "xtask/src/check_tool_grants.rs",
        "checks the counts concepts/ states when present",
    ),
    (
        "xtask/src/check_bo4e_examples.rs",
        "scans concepts/ and specs/ examples when present",
    ),
    ("xtask/src/main.rs", "the help text of check-specs"),
    ("justfile", "the recipe that runs check-specs"),
    (
        "services/makod/tests/it/published_counts.rs",
        "pins counts concepts/ states when present",
    ),
    (
        "services/makod/tests/it/process_coverage.rs",
        "reads the coverage inventory when present",
    ),
    (
        "services/makod/tests/it/occupancy_verdicts.rs",
        "reads the coverage inventory when present",
    ),
    (
        "services/makod/tests/it/startup_smoke.rs",
        "pins counts concepts/ states when present",
    ),
];

/// Trees rule 1 scans.
const PHRASE_TREES: &[&str] = &["crates/", "services/", "site/", "demos/", "makotest/"];

/// Extensions rule 1 reads.
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "md", "sql", "cedar", "toml", "yaml", "yml", "sh", "py", "html", "txt",
];

/// The strings rule 2 refuses.
const POINTERS: &[&str] = &["ROADMAP.md", "concepts/", "specs/"];

/// One offending line: path, 1-based line, message.
type Finding = (String, usize, String);

/// Scan the workspace. Returns `true` when nothing is refused.
pub fn run(workspace_root: &Path) -> bool {
    let Some(files) = candidate_files(workspace_root) else {
        eprintln!(
            "check-known-wrong: `git ls-files` failed under {} — no file could be listed",
            workspace_root.display()
        );
        return false;
    };
    let compiled = compile();
    let mut findings = table_findings(workspace_root);
    let mut scanned = 0usize;
    for rel in &files {
        let Ok(src) = std::fs::read_to_string(workspace_root.join(rel)) else {
            continue;
        };
        scanned += 1;
        if in_phrase_scope(rel) {
            findings.extend(phrase_findings(&compiled, rel, &src));
        }
        findings.extend(pointer_findings(rel, &src));
    }
    if scanned == 0 {
        eprintln!("check-known-wrong: the scan read no file — the layout has probably changed");
        return false;
    }
    if findings.is_empty() {
        println!(
            "check-known-wrong: {scanned} file(s) carry no known-wrong claim from {} phrase \
             rule(s) and no pointer into the untracked planning trees",
            RULES.len()
        );
        return true;
    }
    eprintln!("check-known-wrong: {} finding(s):", findings.len());
    for (path, line, msg) in &findings {
        eprintln!("  {path}:{line}  {msg}");
    }
    false
}

/// Every known-wrong phrase rule `text` matches, as `(line, message)` — for a
/// string that is about to become a claim elsewhere, such as the source of an
/// operator policy option.
#[must_use]
pub fn findings_in_text(text: &str) -> Vec<(usize, String)> {
    phrase_findings(&compile(), "", text)
        .into_iter()
        .map(|(_, line, msg)| (line, msg))
        .collect()
}

/// Every tracked or not-ignored file, workspace-relative.
fn candidate_files(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let list = String::from_utf8_lossy(&out.stdout);
    let mut files: Vec<String> = list
        .split('\0')
        .filter(|f| !f.is_empty())
        .filter(|f| !f.ends_with(".json") && !f.ends_with(".lock"))
        .filter(|f| root.join(f).is_file())
        .map(str::to_owned)
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

fn in_phrase_scope(rel: &str) -> bool {
    let root_markdown = !rel.contains('/') && rel.ends_with(".md");
    root_markdown
        || PHRASE_TREES.iter().any(|t| rel.starts_with(t))
            && Path::new(rel)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| TEXT_EXTENSIONS.contains(&e))
}

fn compile() -> Vec<(&'static Rule, Regex)> {
    RULES
        .iter()
        .map(|r| (r, Regex::new(r.pattern).expect("rule pattern compiles")))
        .collect()
}

/// Emphasis and code ticks stripped, `§` spaced from its number, whitespace
/// collapsed.
fn normalise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            '\u{a0}' | '\u{202f}' | '\u{2009}' | '\t' => out.push(' '),
            '§' => {
                out.push('§');
                while chars.peek() == Some(&'§') {
                    out.push(chars.next().unwrap_or('§'));
                }
                if chars.peek().is_some_and(char::is_ascii_digit) {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut space = false;
    for c in out.chars() {
        if c == ' ' {
            if !space {
                collapsed.push(' ');
            }
            space = true;
        } else {
            collapsed.push(c);
            space = false;
        }
    }
    collapsed
}

/// Rule 1 over one file.
fn phrase_findings(compiled: &[(&'static Rule, Regex)], rel: &str, src: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let table = if rel == "AGENTS.md" {
        known_wrong_table_lines(src)
    } else {
        Vec::new()
    };
    for (i, raw) in src.lines().enumerate() {
        if table.contains(&i) {
            continue;
        }
        let line = normalise(raw);
        for (rule, re) in compiled {
            if !re.is_match(&line) {
                continue;
            }
            let admitted = ALLOWED
                .iter()
                .any(|a| a.path == rel && a.rule == rule.id && line.contains(a.fragment));
            if !admitted {
                out.push((
                    rel.to_owned(),
                    i + 1,
                    format!(
                        "[{}] AGENTS.md known-wrong row „{}\" — {}",
                        rule.id, rule.row, rule.reason
                    ),
                ));
            }
        }
    }
    out
}

/// The 0-based lines of `AGENTS.md`'s known-wrong table: it states every
/// wrong claim, in its first column, so rule 1 skips it there and nowhere else.
fn known_wrong_table_lines(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut in_section = false;
    let mut in_table = false;
    for (i, line) in src.lines().enumerate() {
        if line.starts_with("### Known-wrong legal claims") {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with('|') {
            in_table = true;
            out.push(i);
        } else if in_table {
            break;
        }
    }
    out
}

/// Rule 2 over one file.
fn pointer_findings(rel: &str, src: &str) -> Vec<Finding> {
    if POINTER_SITES.iter().any(|(p, _)| *p == rel) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if let Some(p) = POINTERS.iter().find(|p| line.contains(*p)) {
            out.push((
                rel.to_owned(),
                i + 1,
                format!(
                    "names `{p}` — the roadmap and the specifications are untracked, so a \
                     tracked file must not point into them"
                ),
            ));
        }
    }
    out
}

/// The table's rows, by their first cell.
fn table_rows(agents: &str) -> Vec<String> {
    let Some(start) = agents.find("### Known-wrong legal claims") else {
        return Vec::new();
    };
    agents[start..]
        .lines()
        .skip_while(|l| !l.starts_with("| Wrong claim"))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| l.split(" | ").next())
        .map(|c| c.trim_start_matches("| ").replace('*', ""))
        .collect()
}

/// Every rule names a row, and every row is a rule's or listed as uncovered.
fn table_findings(root: &Path) -> Vec<Finding> {
    let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) else {
        return vec![(
            "AGENTS.md".into(),
            0,
            "the root AGENTS.md is unreadable, so the rows cannot be checked".into(),
        )];
    };
    table_consistency(&table_rows(&agents))
}

fn table_consistency(rows: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    if rows.is_empty() {
        out.push((
            "AGENTS.md".into(),
            0,
            "no Known-wrong legal claims table found — the heading or its header row moved".into(),
        ));
        return out;
    }
    let named = |needle: &str| rows.iter().any(|r| r.contains(&needle.replace('*', "")));
    for rule in RULES {
        if !named(rule.row) {
            out.push((
                "AGENTS.md".into(),
                0,
                format!(
                    "rule [{}] names row „{}\", which the table does not carry",
                    rule.id, rule.row
                ),
            ));
        }
    }
    for row in UNCOVERED {
        if !named(row) {
            out.push((
                "AGENTS.md".into(),
                0,
                format!("UNCOVERED names row „{row}\", which the table does not carry"),
            ));
        }
    }
    for row in rows {
        let covered = RULES
            .iter()
            .map(|r| r.row)
            .chain(UNCOVERED.iter().copied())
            .any(|n| row.contains(&n.replace('*', "")));
        if !covered {
            out.push((
                "AGENTS.md".into(),
                0,
                format!(
                    "known-wrong row „{row}\" is neither a phrase rule nor listed in UNCOVERED"
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(rel: &str, line: &str) -> Vec<String> {
        let compiled = compile();
        phrase_findings(&compiled, rel, line)
            .into_iter()
            .map(|(_, _, m)| m)
            .collect()
    }

    fn hit_ids(line: &str) -> Vec<&'static str> {
        let n = normalise(line);
        compile()
            .into_iter()
            .filter(|(_, re)| re.is_match(&n))
            .map(|(r, _)| r.id)
            .collect()
    }

    /// One planted phrase per covered row, each caught by its own rule.
    #[test]
    fn a_planted_phrase_fails_per_covered_row() {
        let planted: &[(&str, &str)] = &[
            (
                "aperak-weekday",
                "/// APERAK: 45 Minuten on a weekday for a UTILMD",
            ),
            ("aperak-weekday", "// Strom UTILMD (Mo–Fr): 45 min"),
            (
                "aperak-gas-initial-noon",
                "Gas Initialprozess: 3 Werktage, 12 Uhr",
            ),
            (
                "seventeen-cutoff",
                "/// the 17:00 Europe/Berlin MaKo cut-off",
            ),
            (
                "gpke-kap-1-7",
                "Saturday is not a Werktag (GPKE Teil 1 Kap. 1.7).",
            ),
            (
                "asx-sign-only",
                "// a sign-only override is valid AS4 and would send",
            ),
            ("messzv", "/// plausibility per § 22 MessZV"),
            (
                "eeg-20-absatz",
                "/// Jahresmarktwert (§20 Abs. 2 + Anlage 1 EEG 2023)",
            ),
            (
                "enwg-6b-integrated",
                "// (vertically integrated utility — §6b EnWG deployment)",
            ),
            ("stromnev-18-blind", "/// Blindmehrarbeit per §18 StromNEV"),
            ("gasnev-14", "/// Grundpreis (§14 GasNEV)"),
            (
                "eeg-42b",
                "/// Gemeinschaftliche Gebäudeversorgung (§42b EEG 2023)",
            ),
            (
                "enwg-41-3",
                "/// six weeks' notice of a price change (§41 Abs. 3 EnWG)",
            ),
            (
                "eeg-38a-mieterstrom",
                "/// §38a EEG 2023 Mieterstromzuschlag",
            ),
            (
                "msbg-29-100kw",
                "/// §29 MsbG: iMSys mandatory between 7 and 100 kW",
            ),
            ("enwg-40a-abschlag", "/// §40a EnWG Abschlagszahlungen"),
            (
                "enwg-41b-dynamic",
                "/// dynamic tariffs need an iMSys (§41b EnWG)",
            ),
            (
                "eeg-53b-regional",
                "/// §53b regional Grünstromkennzeichnung",
            ),
            (
                "eeg-53c-oversupply",
                "/// §53c structural oversupply reduction",
            ),
            ("eeg-54-generic", "/// §54 EEG reduction for §36d deadlines"),
            ("eeg-35a-erloeschen", "/// Zuschlag expires (§35a EEG)"),
            (
                "eeg-35a-erloeschen",
                "/// BNetzA tender award expired (§33 EEG 2023): plant not built",
            ),
            (
                "eeg-35a-erloeschen",
                "/// - A second tender is required when the first award expires (§33 EEG 2023).",
            ),
            (
                "eeg-35a-erloeschen",
                "/// Zuschlag erlischt (§ 55 EEG 2023)",
            ),
            ("modul-2-3", "/// Modul 2 (HT/NT), Modul 3 (spot)"),
            (
                "invoic-31009-direction",
                "| 31009 | MSB-Rechnung | NB → MSB |",
            ),
            ("pid-13025", "// 13025 is not a real PID"),
            ("ust-12-3-pv", "/// §12 Abs. 3 UStG (Solar ≤ 30 kWp)"),
            (
                "ust-12-3-pv",
                "/// // 50 kWp solar → exceeds the §19 limit, Regelbesteuerung",
            ),
            (
                "ust-12-3-pv",
                "/// // 9.5 kWp solar, commissioned 2024 → Kleinunternehmer (§19)",
            ),
            (
                "ust-12-3-pv",
                "// 50 kWp: exceeds the §19 turnover reality → Regelbesteuerung",
            ),
            ("aperak-ahb-1-0", "/// APERAK AHB 1.0 §2.4.1: 45 Minuten"),
            (
                "fv2027-04-release",
                "pub const RELEASE_2_0_0: &str = \"2.0.0\";",
            ),
            (
                "fv2027-04-release",
                "BNetzA Mitteilung Nr. 57 proposes twelve months",
            ),
            (
                "fv2027-04-release",
                "API-Webdienste 2.0.0 applies ab 01.10.2027",
            ),
            (
                "fv2027-04-release",
                "UTILTS wird durch die API-Webdienste ersetzt",
            ),
            ("fv2027-04-release", "the API retires UTILTS"),
            (
                "role-features-6a",
                "//! Entflechtung of § 6a EnWG, enforced at compile time.",
            ),
            (
                "role-features-6a",
                "a role build contains no other role's code, least privilege in support of § 6a EnWG.",
            ),
            (
                "role-features-6a",
                "# processd's role features enforce § 6a EnWG informatorische Entflechtung",
            ),
        ];
        for rule in RULES {
            assert!(
                planted.iter().any(|(id, _)| *id == rule.id),
                "rule [{}] has no planted phrase",
                rule.id
            );
        }
        for (id, line) in planted {
            assert!(
                hit_ids(line).contains(id),
                "[{id}] must catch: {line} (caught by {:?})",
                hit_ids(line)
            );
        }
    }

    /// The right statements pass.
    #[test]
    fn the_right_statements_pass() {
        for line in [
            "| APERAK | 45 min Strom on every day but Samstag; Gas next Werktag 12:00 (Folgeprozess) or 3 Werktage (Initialprozess) |",
            "/// APERAK AHB 1.1 § 2.4.1: 45 Minuten for a UTILMD or ORDERS",
            "| 55037 | Fall 2 | 8 | **17:00 Uhr am ÜT** von Nr. 1 |",
            "Saturday is not a Werktag (GPKE Teil 1 Kap. 7 Fristenberechnung)",
            "| Kündigungsbestätigung | § 41 Abs. 8 Nr. 2 EnWG, § 20 Abs. 2 GVV |",
            "§ 40a Abs. 2 EnWG (Verbrauchsschätzung) · § 13 Abs. 1 StromGVV (Abschlagshöhe)",
            "| Sonderkündigung Umzug | § 41b Abs. 5 EnWG | 6 Wochen |",
            "//! §18 StromNEV pays the operator of a decentral generating plant",
            "| 13025 | Lastgang Marktlokation, Tranche | MSB → LF |",
            "Modul 1 pauschal, Modul 2 prozentual, Modul 3 zeitvariabel",
            "//! All other operators (large plants exceeding the §19 turnover limit, commercial",
            "/// - Operators whose turnover exceeds the §19 Kleinunternehmer limit",
            "{grenze} Masseprozent — kein Anspruch nach §19 Abs. 1\"",
            "/// Ausschluss von Geboten (§ 33 EEG 2023) before any award exists",
            "/// Zuschlag erlischt (§ 36e EEG 2023) when the plant is not commissioned",
            "/// § 6a EnWG informatorisches Unbundling: in a combined-role (VIU) deployment",
            "| § 6a EnWG informatorisches Unbundling | Cedar on the `ReadProcess` action |",
        ] {
            assert!(
                hit_ids(line).is_empty(),
                "must pass: {line} ({:?})",
                hit_ids(line)
            );
        }
    }

    /// Emphasis and an unspaced `§` are the same claim.
    #[test]
    fn emphasis_and_spacing_do_not_hide_a_claim() {
        assert!(hit_ids("**§14 GasNEV**").contains(&"gasnev-14"));
        assert!(hit_ids("`§ 42b EEG`").contains(&"eeg-42b"));
        assert!(hit_ids("**45 Minuten** on a **weekday**").contains(&"aperak-weekday"));
    }

    /// An allowlist entry admits its sentence and no other in the same file.
    #[test]
    fn an_allowance_is_pinned_to_its_sentence() {
        assert!(
            hits(
                "crates/mako-engine/src/erc.rs",
                "APERAK AHB 1.0 admits 27 for 29001"
            )
            .is_empty()
        );
        assert_eq!(
            hits("crates/mako-engine/src/erc.rs", "/// APERAK AHB 1.0 §2.4.1").len(),
            1,
            "a second, unadmitted occurrence in the same file still fails"
        );
        assert_eq!(
            hits("crates/other.rs", "APERAK AHB 1.0 admits 27 for 29001").len(),
            1,
            "the allowance is per file"
        );
    }

    /// Every allowance still names a rule that exists.
    #[test]
    fn every_allowance_names_a_rule_and_a_reason() {
        for a in ALLOWED {
            assert!(
                RULES.iter().any(|r| r.id == a.rule),
                "{} names no rule",
                a.rule
            );
            assert!(!a.reason.is_empty() && !a.fragment.is_empty());
        }
    }

    #[test]
    fn a_pointer_into_the_planning_trees_is_refused() {
        assert_eq!(
            pointer_findings("services/x/policy.cedar", "// tracked in ROADMAP.md").len(),
            1
        );
        assert_eq!(
            pointer_findings("crates/x/src/lib.rs", "/// see concepts/FORMAT.md").len(),
            1
        );
        assert_eq!(
            pointer_findings("site/content/x.md", "per specs/026-x/spec.md").len(),
            1
        );
        assert!(pointer_findings(".gitignore", "concepts/\nspecs/").is_empty());
        assert!(pointer_findings("crates/x/src/lib.rs", "/// a roadmap of the rollout").is_empty());
    }

    /// The live table: every rule names a row, every row is decided.
    #[test]
    fn the_rules_and_the_table_agree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let found = table_findings(&root);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_new_row_without_a_decision_fails() {
        let mut rows: Vec<String> = RULES.iter().map(|r| r.row.to_owned()).collect();
        rows.extend(UNCOVERED.iter().map(|s| (*s).to_owned()));
        assert!(table_consistency(&rows).is_empty());
        rows.push("\"A brand-new wrong claim\"".into());
        assert_eq!(table_consistency(&rows).len(), 1);
    }

    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent/mako/workspace/root")));
    }
}
