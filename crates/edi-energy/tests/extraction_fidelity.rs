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
//!   condition. The worst class: `remadv [493]` runs to 1 899 characters of
//!   INVOIC commentary where a one-sentence Bedingung belongs.
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
                Some(Voraussetzung::ElementValue { .. }) => as_value += 1,
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
/// Measured 2026-09-14: **179 of 371**. 30 fall back to the segment's mere
/// presence and 162 do not parse at all. What remains needs Voraussetzung
/// variants that do not exist yet — a length test („genau 11 Stellen"), a join
/// between two places, and the ID-format semantics behind „die ID der
/// Marktlokation". Each is a design decision, not a missing branch.
///
/// The floor only rises. It went 78 → 179 when [`Voraussetzung::parse`] learned
/// the comparison shape („Wenn in diesem STS DE1131 = E_0526"), which the whole
/// IFTSTA Antwortcode family is written in and which carries no „vorhanden" for
/// the old gate to catch.
const ELEMENT_VALUE_FLOOR: usize = 179;

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
