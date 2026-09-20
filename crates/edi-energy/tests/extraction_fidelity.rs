//! How much of the AHB survives the reconstruction, measured.
//!
//! Every conformance claim mako makes is downstream of one question: does the
//! profile match the PDF it was read from? Nobody publishes the AHB
//! machine-readably, so the profiles come out of `pdftotext -layout` by
//! geometry — and geometry fails quietly. A German sentence that lost its tail
//! still reads like a German sentence, and a Bedingung that absorbed a page of
//! narrative still parses as JSON. Neither fails a schema.
//!
//! So the loss is measured here rather than asserted in prose, and the numbers
//! are pinned so the next import run has to move them deliberately.
//!
//! What each signal means:
//!
//! - **cited but not captured** — a rule references `[n]` and no text for `[n]`
//!   exists. This is the one unconditional failure: the rule cannot be read at
//!   all. It is zero and must stay zero.
//! - **duplicated phrase** — the wrap repair fired twice and the seam is
//!   visible („in der Rolle LF in der Rolle LF").
//! - **ends on a conjunction** — the sentence stops on „und" / „der" / „die",
//!   so the column ended before the clause did.
//! - **unbalanced parentheses** — an opened bracket the extraction never closed.
//! - **prose bleed** — narrative body text from the page landed inside a
//!   condition, so a one-sentence Bedingung carries a paragraph of the page it
//!   was printed on. The longest condition surviving anywhere in the corpus is
//!   `mscons [130]` at 606 characters, against a median of 72.
//!
//! The detectors are deliberately conservative. A general „truncated mid-word"
//! measure is *not* obtainable: German sentences legitimately end in two-letter
//! role codes („… in der Rolle NB"), which is indistinguishable from a cut. The
//! counts below are therefore a lower bound on the damage, not an estimate of
//! it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use edi_energy::profile::conditions::Voraussetzung;

/// `(release, condition key)` — a condition is only unique within its profile.
type Site = (String, String);

fn profiles_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/profiles"))
}

/// Every shipped profile as `(release label, parsed ahb.json)`.
fn profiles() -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    let root = profiles_root();
    for ty in std::fs::read_dir(&root).expect("profiles dir").flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        let ty_name = ty.file_name().to_string_lossy().into_owned();
        for release in std::fs::read_dir(ty.path()).into_iter().flatten().flatten() {
            let Ok(raw) = std::fs::read_to_string(release.path().join("ahb.json")) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let label = format!("{ty_name}/{}", release.file_name().to_string_lossy());
            out.push((label, v));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        out.len() >= 30,
        "read only {} profiles — the layout changed and this measurement is \
         looking at nothing",
        out.len()
    );
    out
}

/// The `conditions` map of one profile, whitespace-normalised.
fn conditions(v: &serde_json::Value) -> BTreeMap<String, String> {
    v["conditions"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, val)| {
            let t = val.as_str()?;
            Some((
                k.clone(),
                t.split_whitespace().collect::<Vec<_>>().join(" "),
            ))
        })
        .collect()
}

/// Every `[n]` cited by any status expression in the profile.
fn cited(v: &serde_json::Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_citations(&v["anwendungsfaelle"], &mut out);
    out
}

fn collect_citations(v: &serde_json::Value, out: &mut BTreeSet<String>) {
    match v {
        serde_json::Value::String(s) => {
            let b = s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'[' {
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > start && j < b.len() && b[j] == b']' {
                        out.insert(s[start..j].to_owned());
                    }
                }
                i += 1;
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_citations(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_citations(x, out)),
        _ => {}
    }
}

// ---------------------------------------------------------------- detectors

/// An adjacent repeat of a 3–7 word phrase — the wrap repair firing twice.
fn has_duplicated_phrase(t: &str) -> bool {
    let w: Vec<&str> = t.split(' ').collect();
    for n in 3..=7 {
        if w.len() < 2 * n {
            continue;
        }
        for i in 0..=w.len() - 2 * n {
            if w[i..i + n] == w[i + n..i + 2 * n] {
                return true;
            }
        }
    }
    false
}

/// Words a German clause never ends on — so the column ended first.
const DANGLING: &[&str] = &[
    "und", "oder", "der", "die", "das", "mit", "im", "in", "nach", "bei",
];

fn ends_on_conjunction(t: &str) -> bool {
    t.rsplit(' ')
        .next()
        .is_some_and(|w| DANGLING.contains(&w.to_lowercase().as_str()))
}

fn unbalanced_parens(t: &str) -> bool {
    t.matches('(').count() != t.matches(')').count()
}

/// German abbreviations that legitimately put a period mid-sentence.
const ABBR: &[&str] = &[
    "ggf", "ggfs", "bzw", "max", "min", "mind", "gem", "inkl", "exkl", "ca", "evtl", "vgl", "zzgl",
    "abzgl", "sog", "insb", "entspr", "jew", "lt", "usw", "etc", "bspw", "ff", "dgl",
];

/// Narrative body text spliced into a condition.
///
/// The marker is a sentence-final period followed by a lowercase word. German
/// capitalises both nouns and sentence openings, so `„… dar. der einzelnen …"`
/// can only be two fragments joined by the extractor. Multi-token
/// abbreviations („d. h.", „z. B.") are removed first, single-token ones are
/// excluded by name.
///
/// This under-reports: bleed into a capitalised noun — `partin [508]` runs
/// „… durchführen zu können. Kommunikationsdaten und …" — is invisible to it.
fn prose_bleed(t: &str) -> bool {
    let mut s = t.to_owned();
    for multi in ["d. h.", "z. B.", "u. a.", "i. d. R.", "o. g.", "s. o."] {
        s = s.replace(multi, " ");
    }
    let w: Vec<&str> = s.split(' ').collect();
    w.windows(2).any(|p| {
        let (a, b) = (p[0], p[1]);
        let Some(stem) = a.strip_suffix('.') else {
            return false;
        };
        stem.len() >= 2
            && stem.chars().all(|c| c.is_alphabetic())
            && stem.chars().next().is_some_and(char::is_lowercase)
            && !ABBR.contains(&stem.to_lowercase().as_str())
            && b.chars().next().is_some_and(char::is_lowercase)
            && b.chars().filter(|c| c.is_alphabetic()).count() >= 2
    })
}

/// The alphabetic core of a token, with the AHB's punctuation peeled off.
fn core_of(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphabetic())
}

/// A word the column split across a line break, confirmed by the corpus.
///
/// `Nachrichtenempfänge r`, `Verwendungszeitra um`. No dictionary is needed: the
/// profiles are their own evidence, and the correct spelling appears in them
/// many times over while the fragment appears only where the wrap put it. The
/// joined form must be common *and* clearly beat the left fragment, which no
/// ordinary word sequence does — `Wirkarbeit und` would need the corpus to hold
/// `Wirkarbeitund`.
///
/// `xtask`'s importer repairs these against the whole AHB, which is the larger
/// corpus; what survives here is what even that could not confirm.
fn split_word(t: &str, vocab: &BTreeMap<String, usize>) -> bool {
    let w: Vec<&str> = t.split_whitespace().collect();
    w.windows(2).any(|p| {
        let (a, b) = (core_of(p[0]), core_of(p[1]));
        if a.chars().count() < 5 || b.chars().count() > 5 || b.is_empty() {
            return false;
        }
        if !a.ends_with(char::is_lowercase) || !b.starts_with(char::is_lowercase) {
            return false;
        }
        if !a.chars().all(char::is_alphabetic) || !b.chars().all(char::is_alphabetic) {
            return false;
        }
        let seen = vocab.get(&format!("{a}{b}")).copied().unwrap_or(0);
        seen >= 3 && seen > 2 * vocab.get(a).copied().unwrap_or(0)
    })
}

/// Every alphabetic word in every shipped condition, with how often it occurs.
fn condition_vocabulary() -> BTreeMap<String, usize> {
    let mut vocab: BTreeMap<String, usize> = BTreeMap::new();
    for (_, v) in profiles() {
        for text in conditions(&v).values() {
            for w in text.split_whitespace() {
                let w = w.trim_matches(|c: char| !c.is_alphabetic());
                if !w.is_empty() && w.chars().all(char::is_alphabetic) {
                    *vocab.entry(w.to_owned()).or_default() += 1;
                }
            }
        }
    }
    vocab
}

/// Which damage signals a condition text carries.
fn signals(t: &str, vocab: &BTreeMap<String, usize>) -> Vec<&'static str> {
    let mut out = Vec::new();
    if split_word(t, vocab) {
        out.push("split word");
    }
    if has_duplicated_phrase(t) {
        out.push("duplicated phrase");
    }
    if ends_on_conjunction(t) {
        out.push("ends on a conjunction");
    }
    if unbalanced_parens(t) {
        out.push("unbalanced parentheses");
    }
    if prose_bleed(t) {
        out.push("prose bleed");
    }
    out
}

/// Every damaged condition across all profiles.
fn damaged() -> BTreeMap<Site, Vec<&'static str>> {
    let mut out = BTreeMap::new();
    let vocab = condition_vocabulary();
    for (label, v) in profiles() {
        for (key, text) in conditions(&v) {
            let s = signals(&text, &vocab);
            if !s.is_empty() {
                out.insert((label.clone(), key), s);
            }
        }
    }
    out
}

// ------------------------------------------------------------------- tests

/// The one unconditional failure: a rule nobody can read.
#[test]
fn no_rule_cites_a_condition_whose_text_was_lost() {
    let mut missing: Vec<String> = Vec::new();
    let mut captured = 0usize;
    let mut citations = 0usize;
    for (label, v) in profiles() {
        let have = conditions(&v);
        captured += have.len();
        let want = cited(&v);
        citations += want.len();
        for key in want {
            if !have.contains_key(&key) {
                missing.push(format!("{label} [{key}]"));
            }
        }
    }
    println!("condition citations: {citations}, conditions captured: {captured}");
    assert!(
        missing.is_empty(),
        "{} rule(s) cite a Bedingung whose text the extraction lost, so the rule \
         cannot be read at all: {missing:?}",
        missing.len()
    );
}

/// The damage budget. It may fall; it may not rise without a decision.
///
/// Measured 2026-09-14 over the shipped profiles: 18 duplicated phrase,
/// 11 ending on a conjunction, 8 unbalanced parentheses, 8 split word,
/// 2 prose bleed — **46 distinct conditions** of 4 306 (1.1 %), some carrying
/// more than one signal.
///
/// The figure went 38 → 46 when `split word` was added, which is more
/// *measurement*, not more damage: the importer repairs that class against the
/// whole AHB and 8 survive it. A budget rise is only ever a decision, so it is
/// recorded here rather than absorbed.
const DAMAGE_BUDGET: usize = 46;

#[test]
fn condition_damage_stays_within_budget() {
    let dmg = damaged();
    let mut by_signal: BTreeMap<&str, usize> = BTreeMap::new();
    for sigs in dmg.values() {
        for s in sigs {
            *by_signal.entry(s).or_default() += 1;
        }
    }
    for (s, n) in &by_signal {
        println!("{s:24}: {n}");
    }
    println!("{:24}: {}", "distinct conditions", dmg.len());

    assert!(
        dmg.len() <= DAMAGE_BUDGET,
        "condition damage rose to {} (budget {DAMAGE_BUDGET}). An import run made \
         the reconstruction worse. Newly damaged: {:?}",
        dmg.len(),
        dmg.keys().collect::<Vec<_>>()
    );
    assert!(
        dmg.len() + 10 >= DAMAGE_BUDGET,
        "condition damage fell to {} — good, but lower DAMAGE_BUDGET to match so \
         the guard keeps its grip",
        dmg.len()
    );
}

/// Damaged text that gates a `Muss` or `Soll` is the part that can change a
/// verdict. An entry here has been read against the PDF and found legible.
///
/// Both entries are the same condition in two Formatversionen, and both are a
/// false positive of the duplicate-phrase detector rather than damage.
const GATED_BUT_READ: &[(&str, &str, &str)] = &[
    (
        "pricat/fv20251001",
        "10",
        "flagged for \u{201e}von LIN DE7140 von LIN DE7140\u{201c}, which is grammatical German \
         (\u{201e}sich der Inhalt von A von B unterscheidet\u{201c}) \u{2014} a false positive of the \
         duplicate-phrase detector, not a wrap repair. Verified against PRICAT AHB.",
    ),
    (
        "pricat/fv20261001",
        "10",
        "same condition, next Formatversion",
    ),
];

#[test]
fn no_muss_or_soll_rests_on_unread_damaged_text() {
    let dmg = damaged();
    let allowed: BTreeSet<Site> = GATED_BUT_READ
        .iter()
        .map(|(r, k, _)| ((*r).to_owned(), (*k).to_owned()))
        .collect();

    let mut gated: BTreeSet<Site> = BTreeSet::new();
    for (label, v) in profiles() {
        let mine: BTreeSet<&String> = dmg
            .keys()
            .filter(|(r, _)| r == &label)
            .map(|(_, k)| k)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut exprs = BTreeSet::new();
        collect_status_expressions(&v["anwendungsfaelle"], &mut exprs);
        for e in exprs {
            let head = e.trim_start();
            if !(head.starts_with("Muss")
                || head.starts_with("Soll")
                || head.starts_with("M ")
                || head.starts_with("S ")
                || head == "M"
                || head == "S")
            {
                continue;
            }
            let mut refs = BTreeSet::new();
            collect_citations(&serde_json::Value::String(e.clone()), &mut refs);
            for r in refs {
                if mine.contains(&r) {
                    gated.insert((label.clone(), r));
                }
            }
        }
    }

    let unread: Vec<&Site> = gated.difference(&allowed).collect();
    assert!(
        unread.is_empty(),
        "{} Muss/Soll place(s) are gated by a condition the extraction damaged and \
         nobody has read: {unread:?}. Read each against the PDF, then either fix \
         the import or record it in GATED_BUT_READ with what you found.",
        unread.len()
    );

    let stale: Vec<&Site> = allowed.difference(&gated).collect();
    assert!(
        stale.is_empty(),
        "GATED_BUT_READ names {stale:?}, which no longer gate a Muss/Soll. Drop \
         them — an allow-list that over-states the exposure trains readers to \
         ignore it."
    );
}

fn collect_status_expressions(v: &serde_json::Value, out: &mut BTreeSet<String>) {
    match v {
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_status_expressions(x, out)),
        serde_json::Value::Object(o) => {
            for (k, val) in o {
                if matches!(k.as_str(), "operand" | "status")
                    && let Some(s) = val.as_str()
                {
                    out.insert(s.to_owned());
                }
                collect_status_expressions(val, out);
            }
        }
        _ => {}
    }
}

/// The gap that is *downstream* of extraction: a condition whose text survived
/// intact but which the evaluator cannot read as what it says.
///
/// A `Wenn … DExxxx … vorhanden` Bedingung is a statement about an element's
/// **value**. When [`Voraussetzung::parse`] falls back to the segment's mere
/// presence, the rule fires for messages the element does not select; when it
/// does not parse at all, the rule is not evaluated. Both directions are wrong,
/// and neither announces itself.
#[test]
fn element_value_conditions_are_read_as_values() {
    let mut total = 0usize;
    let mut as_value = 0usize;
    let mut as_presence = 0usize;
    let mut unparsed: Vec<String> = Vec::new();

    let mut seen = BTreeSet::new();
    for (label, v) in profiles() {
        for (key, text) in conditions(&v) {
            if !mentions_de(&text) || !text.to_lowercase().starts_with("wenn ") {
                continue;
            }
            if !seen.insert(text.clone()) {
                continue; // the same Bedingung ships in several releases
            }
            total += 1;
            match Voraussetzung::parse(&text) {
                // Both read the element the Bedingung names: `ElementValue`
                // against a code list, `ElementShape` against the value's
                // form. What the count separates is reading the element from
                // settling for the segment around it.
                Some(Voraussetzung::ElementValue { .. } | Voraussetzung::ElementShape { .. }) => {
                    as_value += 1;
                }
                Some(_) => as_presence += 1,
                None => unparsed.push(format!("{label} [{key}]")),
            }
        }
    }
    println!(
        "DE-referencing Wenn-conditions: {total} — as element value: {as_value}, \
         as segment presence: {as_presence}, unparsed: {}",
        unparsed.len()
    );
    assert!(
        total >= 200,
        "found only {total} DE-referencing conditions — the corpus or the shape \
         changed and this measurement is looking at nothing"
    );
    // A clause that names a data element is about that element. Reading it as
    // the segment around it answers a different question and fires the rule
    // for messages the element never selected — „Wenn im selben SG12 NAD
    // DE3124 nicht vorhanden" becomes „no NAD at all", which is false in
    // almost every message and so inverts the rule. `Voraussetzung::parse`
    // returns `None` where it cannot read the element, and `None` permits.
    assert_eq!(
        as_presence, 0,
        "{as_presence} DE-referencing conditions are read as the mere presence \
         of their segment; that is the wrong question and it fires for messages \
         the element does not select. Read the element, or return `None` and let \
         it permit"
    );
    assert!(
        as_value >= ELEMENT_VALUE_FLOOR,
        "only {as_value} of {total} DE-referencing conditions parse as an element \
         value (floor {ELEMENT_VALUE_FLOOR}); {as_presence} fall back to segment \
         presence and {} do not parse. A rule read as segment presence fires for \
         messages the element does not select, and one that does not parse is \
         never evaluated — both are silent.",
        unparsed.len()
    );
    assert!(
        as_value <= ELEMENT_VALUE_FLOOR + 15,
        "element-value coverage rose to {as_value} — good; raise \
         ELEMENT_VALUE_FLOOR to match so the guard keeps its grip"
    );
}

/// How many DE-referencing conditions the evaluator reads as what they say.
///
/// **237 of 371**, and **none** falls back to the segment's mere presence —
/// that reading is refused outright, because a clause naming a data element is
/// about that element and reading it as the segment around it inverts the rule.
/// The remaining 134 do not parse. Most of them are not a parser gap at all: an
/// MP-ID's Sparte or Rolle is a property of the recipient the message does not
/// carry, and a code list the AHB names without printing belongs to the
/// Codelisten import. What is genuinely missing is a shape — a join between two
/// places, and matching two segments by a Zeitraum-ID they share.
///
/// The floor only rises, and it is a floor rather than a target because each
/// shape below was found by reading texts the evaluator had silently permitted:
///
/// - the **comparison** („Wenn in diesem STS DE1131 = E_0526"), which the whole
///   IFTSTA Antwortcode family is written in and which carries no „vorhanden"
///   for a presence gate to catch;
/// - three readings the **page** rather than the grammar defeats: a code list
///   the Bedingungen column wrapped after a `/` („DE4465 = A01/A21/A22/
///   A23/A90/A96"), a comparison printed without spaces („DE4465=28"), and the
///   Meldepunkt's own question — „die ID einer Marktlokation angegeben ist",
///   „genau 11 Stellen" — which [`edi_energy::profile::formatbedingung`]
///   answers;
/// - a code named **before** its DE („der Code Z35 … im DE1153"), a dashed
///   Artikel-ID, „mit 1 vorhanden", and the bare question whether an element
///   carries a value at all;
/// - a **path** („in dieser SG8 SEQ+Z01 SG10 CCI+++ZA6 … CAV+E02 vorhanden")
///   and the alternatives the AHB writes with „oder".
const ELEMENT_VALUE_FLOOR: usize = 237;

/// Whether the text names a data element (`DE` followed by digits).
fn mentions_de(t: &str) -> bool {
    t.match_indices("DE").any(|(i, _)| {
        t[i + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

#[test]
#[ignore = "diagnostic: prints what the evaluator cannot read"]
fn sample_unparsed() {
    let mut seen = BTreeSet::new();
    let mut n = 0;
    for (label, v) in profiles() {
        for (key, text) in conditions(&v) {
            if !mentions_de(&text) || !text.to_lowercase().starts_with("wenn ") {
                continue;
            }
            if !seen.insert(text.clone()) {
                continue;
            }
            if Voraussetzung::parse(&text).is_none() && n < 25 {
                n += 1;
                println!(
                    "{label} [{key}] {}",
                    &text.chars().take(150).collect::<String>()
                );
            }
        }
    }
}

/// How thinly each Prüfidentifikator is actually validated.
///
/// A Bedingung the evaluator cannot read is `Truth::Unknown`, which permits and
/// never requires ([FORMAT.md] § 6). That invariant is honest, and it is also
/// the one number a buyer would ask about the wrong way round: the count of
/// unreadable *conditions* says nothing about whether a given Anwendungsfall is
/// checked, because one unreadable Bedingung may gate forty places in one PID
/// and none in the next.
///
/// So this counts **binding places** — a row or data element the column marks
/// `Muss`/`X`/`M` — whose status cites at least one Voraussetzung nothing can
/// read, and reports them per PID. Those places are admitted whatever the
/// message carries.
///
/// [FORMAT.md]: ../../../concepts/FORMAT.md
#[test]
fn the_silent_permit_is_measured_per_pruefidentifikator() {
    let mut per_pid: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut places = 0usize;

    for (label, v) in profiles() {
        let conds = conditions(&v);
        // A Voraussetzung is readable when its text is there and parses. A
        // Bedingung that states a constraint rather than a precondition does
        // not gate the place at all, so it is not a silent permit.
        let unreadable = |id: &str| -> bool {
            match conds.get(id) {
                None => true,
                Some(text) => {
                    edi_energy::profile::conditions::is_precondition(text)
                        && Voraussetzung::parse(text).is_none()
                }
            }
        };
        let gated = |expr: &str| -> bool {
            edi_energy::profile::conditions::Status::parse(expr)
                .and_then(|s| s.expr)
                .is_some_and(|e| {
                    e.cited().into_iter().any(|id| {
                        edi_energy::profile::conditions::ConditionKind::of(id)
                            == edi_energy::profile::conditions::ConditionKind::Voraussetzung
                            && unreadable(id)
                    })
                })
        };
        for af in v["anwendungsfaelle"].as_array().into_iter().flatten() {
            let pid = af["pid"].as_u64().map_or_else(
                || af["name"].as_str().unwrap_or("?").to_owned(),
                |p| p.to_string(),
            );
            let key = format!("{label} {pid}");
            let mut thin = 0usize;
            for row in af["rows"].as_array().into_iter().flatten() {
                for st in row["status"].as_array().into_iter().flatten() {
                    let Some(st) = st.as_str() else { continue };
                    if !st.starts_with("Muss") {
                        continue;
                    }
                    places += 1;
                    if gated(st) {
                        thin += 1;
                    }
                }
            }
            for el in af["elements"].as_array().into_iter().flatten() {
                for op in el["operands"].as_array().into_iter().flatten() {
                    let Some(op) = op["operand"].as_str() else {
                        continue;
                    };
                    if !matches!(op.split_whitespace().next(), Some("X" | "M")) {
                        continue;
                    }
                    places += 1;
                    if gated(op) {
                        thin += 1;
                    }
                }
            }
            total += thin;
            if thin > 0 {
                per_pid.insert(key, thin);
            }
        }
    }

    let mut worst: Vec<(&String, &usize)> = per_pid.iter().collect();
    worst.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    println!(
        "binding places gated by an unreadable Voraussetzung: {total} of {places}, \
         in {} of the shipped Anwendungsfälle. Worst:",
        per_pid.len()
    );
    for (key, n) in worst.iter().take(15) {
        println!("  {n:>4}  {key}");
    }

    assert!(
        places > 20_000,
        "found only {places} binding places — the corpus or the shape changed \
         and this measurement is looking at nothing"
    );
    assert!(
        total <= SILENT_PERMIT_BUDGET,
        "places admitted whatever they carry rose to {total} (budget \
         {SILENT_PERMIT_BUDGET}). A place gated by a Bedingung nothing can read \
         is not validated; teach `Voraussetzung::parse` the shape, or say in \
         ROADMAP.md why it cannot be read"
    );
    assert!(
        total + 200 >= SILENT_PERMIT_BUDGET,
        "places admitted whatever they carry fell to {total} — good; lower \
         SILENT_PERMIT_BUDGET to match so the guard keeps its grip"
    );
}

/// Binding places gated by a Voraussetzung nothing can read.
///
/// The budget only falls. It is not the count of unreadable Bedingungen: one
/// of those can gate many places in one Anwendungsfall and none in the next,
/// which is why the figure that matters is per place and is reported per PID.
const SILENT_PERMIT_BUDGET: usize = 2203;
