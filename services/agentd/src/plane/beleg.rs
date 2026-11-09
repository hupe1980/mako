//! Belegprüfung: whether a sentence a model wrote is backed by the tool output
//! it cites.
//!
//! A model-backed answer is a list of sentences, each naming the run step it
//! rests on. Every figure a sentence states — an identifier, a date, a time of
//! day, a number — must occur in that step's output, or the sentence is
//! `belegt: false`. The check is code, not a second model: a second model
//! would be a disagreement, not a proof.
//!
//! `belegt` says the figures occur in what the agent read. It says nothing
//! about whether the reasoning between them is right, and a figure the model
//! derived — a count of rows, a difference of two dates, „in 3 Tagen" — is not
//! in the step and so is not belegt. That is the point: a derived figure is
//! the service's to compute.
//!
//! Both sides are reduced to one canonical form before they are compared: a
//! German date `01.12.2026` and the ISO `2026-12-01` are one date, `1.234,56`
//! and `1234.56` one number, and an instant a service returns in UTC also
//! counts as its Berlin date and Berlin wall-clock time, which is how a
//! sentence for a German reader states it.
//!
//! ## A rule is belegt only by its quote
//!
//! A sentence naming a § or a chapter — „§ 41f Abs. 5 EnWG", „APERAK AHB 1.1
//! Kap. 2.4.1" — states a rule, and a rule is belegt only beside the passage
//! that states it: the sentence cites `agentd/quote_source`, and one quote of
//! the run holds every address the sentence names, document, edition where
//! the sentence names one, and paragraph or chapter
//! ([`belegt_im_lauf`]). A quote of Abs. 1 does not carry a sentence about
//! Abs. 5, and a chapter whose document the sentence does not name has no
//! address at all, so it is never belegt.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime};

/// One figure, canonical.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Figure {
    /// A calendar date.
    Date(Date),
    /// A wall-clock time, hours and minutes.
    Time(u8, u8),
    /// A number or an all-digit identifier, as its canonical decimal string.
    Number(String),
    /// An identifier with letters (a MeLo-ID, a Vorgangsnummer), exactly.
    Id(String),
}

/// One sentence of a model's answer and the step it cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Satz {
    pub text: String,
    /// The run step the sentence rests on, or `None`.
    pub cites: Option<String>,
}

/// Whether `satz` is backed by `step` — the cited step's output as JSON, or
/// `None` when the sentence cites nothing or a step the run does not have.
///
/// A sentence citing nothing is never belegt, even one without a figure: an
/// uncited sentence is unchecked text, and the Desk marks it as such.
#[must_use]
pub fn belegt(satz: &Satz, step: Option<&serde_json::Value>) -> bool {
    let (Some(_), Some(step)) = (&satz.cites, step) else {
        return false;
    };
    let mut read = BTreeSet::new();
    collect_json(step, &mut read);
    figures(&satz.text).iter().all(|f| read.contains(f))
}

/// The cited call a sentence naming a rule must rest on.
pub const ZITAT_QUELLE: &str = "agentd/quote_source";

/// Whether `satz` is belegt within its run: `step` the output of the call it
/// cites, `zitate` the output of every `quote_source` call the run made.
///
/// A sentence naming no § and no chapter is checked as [`belegt`] checks it.
/// One naming any is belegt only when it cites [`ZITAT_QUELLE`] and a single
/// quote holds every address it names and carries its figures.
#[must_use]
pub fn belegt_im_lauf(
    satz: &Satz,
    step: Option<&serde_json::Value>,
    zitate: &[&serde_json::Value],
) -> bool {
    let adressen = adressen(&satz.text);
    if adressen.is_empty() {
        return belegt(satz, step);
    }
    if satz.cites.as_deref() != Some(ZITAT_QUELLE) {
        return false;
    }
    zitate
        .iter()
        .any(|z| adressen.iter().all(|a| a.passt(z)) && belegt(satz, Some(z)))
}

/// A rule's address as a sentence names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adresse {
    /// The document, or none where the sentence does not name one.
    pub doc: Option<String>,
    /// The edition, where the sentence names one.
    pub edition: Option<String>,
    /// `§ 41f Abs. 5` or `2.4.1`, as the index spells it.
    pub at: String,
}

impl Adresse {
    /// Whether the quote `z` is the passage at this address.
    fn passt(&self, z: &serde_json::Value) -> bool {
        let Some(doc) = &self.doc else {
            return false;
        };
        z["doc"]
            .as_str()
            .is_some_and(|d| d.eq_ignore_ascii_case(doc))
            && z["at"].as_str() == Some(self.at.as_str())
            && self
                .edition
                .as_deref()
                .is_none_or(|e| z["edition"].as_str() == Some(e))
    }
}

/// A chapter: `Kap. 2.4.1`, `Kapitel 3`, or an AHB's own `§ 2.4.1`.
static KAPITEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Kap\.|Kapitel)\s*(\d+(?:\.\d+)*)|§\s*(\d+(?:\.\d+)+)").expect("chapter")
});

/// An EDI@Energy document with its edition: `APERAK AHB 1.1`,
/// `UTILMD MIG Gas G1.2`.
static DOKUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z]+ (?:AHB|MIG)(?: (?:Gas|Strom))?) ([A-Z]?\d+\.\d+[a-z]?)\b")
        .expect("document")
});

/// A statute's paragraph: `§ 41f Abs. 5 S. 1 EnWG`. The Absatz is kept, a
/// Satz or Nummer is not — the index's passage is the Absatz that holds it.
static PARAGRAPH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"§\s*(\d+[a-z]?)(?:\s*(?:Abs\.|Absatz)\s*(\d+[a-z]?))?(?:\s*(?:S\.|Satz)\s*\d+)?(?:\s*Nr\.\s*\d+[a-z]?)?(?:\s+([A-ZÄÖÜ][A-Za-zÄÖÜäöü]*[A-Z][A-Za-z]*))?",
    )
    .expect("paragraph")
});

/// Every rule address `text` names.
#[must_use]
pub fn adressen(text: &str) -> Vec<Adresse> {
    let mut out = Vec::new();
    let mut belegt_spans = Vec::new();
    let docs: Vec<(usize, String, String)> = DOKUMENT
        .captures_iter(text)
        .filter_map(|c| Some((c.get(0)?.start(), c[1].to_owned(), c[2].to_owned())))
        .collect();
    for c in KAPITEL.captures_iter(text) {
        let Some(m) = c.get(0) else { continue };
        belegt_spans.push(m.range());
        let at = c.get(1).or_else(|| c.get(2)).map_or("", |n| n.as_str());
        // The document named last before the chapter, else the first after it.
        let doc = docs
            .iter()
            .rev()
            .find(|(at, ..)| *at < m.start())
            .or_else(|| docs.iter().find(|(at, ..)| *at > m.start()));
        out.push(Adresse {
            doc: doc.map(|(_, d, _)| d.clone()),
            edition: doc.map(|(_, _, e)| e.clone()),
            at: at.to_owned(),
        });
    }
    for c in PARAGRAPH.captures_iter(text) {
        let Some(m) = c.get(0) else { continue };
        if belegt_spans.iter().any(|r| r.contains(&m.start())) {
            continue;
        }
        let at = match c.get(2) {
            Some(abs) => format!("§ {} Abs. {}", &c[1], abs.as_str()),
            None => format!("§ {}", &c[1]),
        };
        out.push(Adresse {
            doc: c.get(3).map(|d| d.as_str().to_owned()),
            edition: None,
            at,
        });
    }
    out
}

/// Every figure a JSON value carries: string values tokenised as text, an
/// RFC 3339 instant as its Berlin date and time only — a sentence stating the
/// UTC wall clock reads to a German reader as Berlin time and is an hour off
/// in winter, two in summer — numbers canonical.
fn collect_json(v: &serde_json::Value, out: &mut BTreeSet<Figure>) {
    match v {
        serde_json::Value::String(s) => {
            if let Ok(instant) = OffsetDateTime::parse(s, &Rfc3339) {
                let berlin = mako_fristen::in_berlin(instant);
                out.insert(Figure::Date(berlin.date()));
                out.insert(Figure::Time(berlin.hour(), berlin.minute()));
            }
            out.extend(figures(s));
        }
        serde_json::Value::Number(n) => {
            if let Some(c) = canonical_number(&n.to_string()) {
                out.insert(Figure::Number(c));
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_json(i, out)),
        serde_json::Value::Object(map) => map.values().for_each(|i| collect_json(i, out)),
        serde_json::Value::Bool(_) | serde_json::Value::Null => {}
    }
}

/// The figures a text states.
fn figures(text: &str) -> BTreeSet<Figure> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | ',' | ':' | '-')))
        .filter_map(|raw| {
            // Sentence punctuation is not part of a figure: „am 01.12.2026."
            let t = raw.trim_matches(|c: char| matches!(c, '.' | ',' | ':' | '-'));
            classify(t)
        })
        .collect()
}

fn classify(t: &str) -> Option<Figure> {
    if t.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(d) = german_date(t).or_else(|| iso_date(t)) {
        return Some(Figure::Date(d));
    }
    if let Some((h, m)) = clock(t) {
        return Some(Figure::Time(h, m));
    }
    if t.chars().any(char::is_alphabetic) {
        // `T09:00:00Z` inside an instant is the instant's, handled above; any
        // other token with letters and digits is an identifier.
        return (t.len() >= 5 && !t.contains(':')).then(|| Figure::Id(t.to_owned()));
    }
    german_number(t)
        .or_else(|| canonical_number(t))
        .map(Figure::Number)
}

/// `dd.mm.yyyy`.
fn german_date(t: &str) -> Option<Date> {
    let mut p = t.split('.');
    let (d, m, y) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || d.len() > 2 || m.len() > 2 || y.len() != 4 {
        return None;
    }
    date(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

/// `yyyy-mm-dd`.
fn iso_date(t: &str) -> Option<Date> {
    let mut p = t.split('-');
    let (y, m, d) = (p.next()?, p.next()?, p.next()?);
    if p.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    date(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

fn date(y: i32, m: u8, d: u8) -> Option<Date> {
    Date::from_calendar_date(y, Month::try_from(m).ok()?, d).ok()
}

/// `HH:MM`.
fn clock(t: &str) -> Option<(u8, u8)> {
    let (h, m) = t.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u8, u8) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// A number in German notation — `1.234,56`, `0,55`, `1.234` — canonical.
/// Returns `None` for a token with no German separator, which
/// [`canonical_number`] reads.
fn german_number(t: &str) -> Option<String> {
    let (int, frac) = match t.split_once(',') {
        Some((i, f)) => (i, Some(f)),
        None => (t, None),
    };
    let grouped = int.contains('.');
    if !grouped && frac.is_none() {
        return None;
    }
    if grouped {
        let mut groups = int.split('.');
        let lead = groups.next()?;
        if lead.is_empty() || lead.len() > 3 || groups.clone().any(|g| g.len() != 3) {
            return None;
        }
    }
    let digits: String = int.chars().filter(|c| *c != '.').collect();
    let joined = match frac {
        Some(f) => format!("{digits}.{f}"),
        None => digits,
    };
    canonical_number(&joined)
}

/// A plain decimal — `1234.56`, `55001`, `-3` — without leading zeros in the
/// integer part or trailing zeros in the fraction. An identifier keeps its
/// leading zeros: a Vorgangsnummer `0042` is not the number 42.
fn canonical_number(t: &str) -> Option<String> {
    let (neg, body) = t.strip_prefix('-').map_or((false, t), |b| (true, b));
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if int.is_empty() || !int.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let frac = frac.trim_end_matches('0');
    let sign = if neg { "-" } else { "" };
    Some(if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn satz(text: &str) -> Satz {
        Satz {
            text: text.to_owned(),
            cites: Some("step-1".to_owned()),
        }
    }

    fn step() -> serde_json::Value {
        json!({
            "business_key": "51238696012",
            "processes": [{
                "workflow_name": "gpke-supplier-change",
                "pid": 55001,
                "deadlines": [{ "label": "gpke-antwortfrist", "due_at": "2026-11-05T10:00:00Z" }]
            }],
            "lieferbeginn": "2026-12-01",
            "betrag": "1234.50"
        })
    }

    #[test]
    fn a_sentence_whose_figures_the_step_carries_is_belegt() {
        let s = satz(
            "Die Anmeldung für 51238696012 (PID 55001) beginnt am 01.12.2026; \
             die Frist endet am 05.11.2026 um 11:00 Uhr, offen sind 1.234,50 €.",
        );
        assert!(belegt(&s, Some(&step())));
    }

    #[test]
    fn a_malo_the_step_does_not_carry_is_not_belegt() {
        assert!(!belegt(
            &satz("Die MaLo 41203768152 ist betroffen."),
            Some(&step())
        ));
    }

    #[test]
    fn an_amount_in_german_notation_that_differs_is_not_belegt() {
        assert!(!belegt(&satz("Offen sind 1.243,50 €."), Some(&step())));
    }

    #[test]
    fn a_date_absent_from_the_step_is_not_belegt() {
        assert!(!belegt(
            &satz("Lieferbeginn ist der 02.12.2026."),
            Some(&step())
        ));
    }

    #[test]
    fn a_derived_figure_is_not_belegt() {
        // The step carries the instant; „in 3 Tagen" is the model's arithmetic.
        assert!(!belegt(&satz("Die Frist endet in 3 Tagen."), Some(&step())));
    }

    #[test]
    fn a_sentence_citing_nothing_is_never_belegt() {
        let s = Satz {
            text: "Alles in Ordnung.".to_owned(),
            cites: None,
        };
        assert!(!belegt(&s, Some(&step())));
        assert!(!belegt(&satz("Alles in Ordnung."), None));
    }

    #[test]
    fn the_berlin_time_of_a_utc_instant_counts() {
        // 10:00 UTC on 5 November is 11:00 in Berlin (CET).
        assert!(belegt(
            &satz("Frist: 05.11.2026, 11:00 Uhr."),
            Some(&step())
        ));
        assert!(!belegt(
            &satz("Frist: 05.11.2026, 12:00 Uhr."),
            Some(&step())
        ));
        // The UTC wall clock is not how the sentence is read.
        assert!(!belegt(
            &satz("Frist: 05.11.2026, 10:00 Uhr."),
            Some(&step())
        ));
    }

    #[test]
    fn a_vorgangsnummer_keeps_its_leading_zeros() {
        let step = json!({ "vorgang": "0042" });
        assert!(belegt(&satz("Vorgang 0042 ist offen."), Some(&step)));
        assert!(!belegt(&satz("Vorgang 42 ist offen."), Some(&step)));
    }

    fn zitat(doc: &str, edition: &str, at: &str, text: &str) -> serde_json::Value {
        json!({ "doc": doc, "edition": edition, "at": at, "text": text, "sha256": "03b3" })
    }

    fn zitiert(text: &str) -> Satz {
        Satz {
            text: text.to_owned(),
            cites: Some(ZITAT_QUELLE.to_owned()),
        }
    }

    const ABS_5: &str = "(5) Der Beginn der Unterbrechung der Energieversorgung ist dem \
        Haushaltskunden acht Werktage im Voraus durch briefliche Mitteilung anzukündigen.";

    #[test]
    fn a_rule_is_belegt_only_beside_the_quote_of_its_own_absatz() {
        let s = zitiert(
            "Die Unterbrechung ist acht Werktage im Voraus anzukündigen (§ 41f Abs. 5 EnWG).",
        );
        let abs1 = zitat(
            "EnWG",
            "Stand 2026-10-22",
            "§ 41f Abs. 1",
            "(1) Bei der Nichterfüllung …",
        );
        let abs5 = zitat("EnWG", "Stand 2026-10-22", "§ 41f Abs. 5", ABS_5);
        assert!(
            !belegt_im_lauf(&s, Some(&abs1), &[&abs1]),
            "a quote of Abs. 1 carries no Abs. 5"
        );
        assert!(belegt_im_lauf(&s, Some(&abs5), &[&abs1, &abs5]));
    }

    #[test]
    fn a_rule_cited_to_a_service_read_is_not_belegt() {
        let abs5 = zitat("EnWG", "Stand 2026-10-22", "§ 41f Abs. 5", ABS_5);
        let s = Satz {
            text: "Nach § 41f Abs. 5 EnWG gilt das.".to_owned(),
            cites: Some("makod/get_process".to_owned()),
        };
        assert!(!belegt_im_lauf(&s, Some(&step()), &[&abs5]));
    }

    #[test]
    fn an_ahb_chapter_needs_its_edition() {
        let s = zitiert("Laut APERAK AHB 1.1 Kap. 2.4.1 gilt die Frist von 45 Minuten.");
        let z10 = zitat("APERAK AHB", "1.0", "2.4.1", "binnen 45 Minuten");
        let z11 = zitat("APERAK AHB", "1.1", "2.4.1", "binnen 45 Minuten");
        assert!(!belegt_im_lauf(&s, Some(&z10), &[&z10]), "1.0 is not 1.1");
        assert!(belegt_im_lauf(&s, Some(&z11), &[&z10, &z11]));
        // The AHB's own `§ 2.4.1` is the same chapter.
        let s = zitiert("APERAK AHB 1.1 § 2.4.1 nennt 45 Minuten.");
        assert!(belegt_im_lauf(&s, Some(&z11), &[&z11]));
    }

    #[test]
    fn a_chapter_without_its_document_is_never_belegt() {
        let z = zitat("APERAK AHB", "1.1", "2.4.1", "binnen 45 Minuten");
        let s = zitiert("Nach Kap. 2.4.1 gilt die Frist von 45 Minuten.");
        assert_eq!(adressen(&s.text)[0].doc, None);
        assert!(!belegt_im_lauf(&s, Some(&z), &[&z]));
    }

    #[test]
    fn a_sentence_naming_no_rule_is_checked_as_before() {
        let s = satz("Die Anmeldung für 51238696012 läuft.");
        assert!(belegt_im_lauf(&s, Some(&step()), &[]));
    }

    #[test]
    fn the_addresses_a_sentence_names() {
        assert_eq!(
            adressen("§ 41f Abs. 5 S. 1 EnWG und § 34 MsbG"),
            vec![
                Adresse {
                    doc: Some("EnWG".into()),
                    edition: None,
                    at: "§ 41f Abs. 5".into()
                },
                Adresse {
                    doc: Some("MsbG".into()),
                    edition: None,
                    at: "§ 34".into()
                },
            ]
        );
        assert_eq!(
            adressen("UTILMD MIG Gas G1.2 Kapitel 3"),
            vec![Adresse {
                doc: Some("UTILMD MIG Gas".into()),
                edition: Some("G1.2".into()),
                at: "3".into()
            }]
        );
    }

    #[test]
    fn german_and_plain_numbers_canonicalise_alike() {
        assert_eq!(german_number("1.234,50").as_deref(), Some("1234.5"));
        assert_eq!(german_number("0,55").as_deref(), Some("0.55"));
        assert_eq!(german_number("1.234").as_deref(), Some("1234"));
        assert_eq!(german_number("12.34"), None, "not a German grouping");
        assert_eq!(canonical_number("1234.50").as_deref(), Some("1234.5"));
    }
}
