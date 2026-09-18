//! Formatbedingungen `[901]`–`[999]` — what the AHB says about a *value*.
//!
//! Allgemeine Festlegungen 6.1d Kap. 6.4.2: Formatbedingungen „beschreiben, in
//! welchem Format der Wert im jeweiligen Datenelement anzugeben ist", and
//! Kap. 6.4: „Die Nummer für eine Formatbedingung ist über alle
//! Nachrichtentypen hinweg eindeutig" — so the same `[931]`
//! means the same thing in UTILMD and in MSCONS. That uniqueness is why this is
//! a registry **keyed on the number** rather than a parser of the German
//! sentence beside it: a new AHB citing `[950]` at a new place is checked for
//! free, and a Bedingungstext mangled by the PDF column (`[969]` is printed
//! „Möglicher **Wer**: ≤ 1") still evaluates.
//!
//! A Formatbedingung never gates whether a place must appear — that is
//! [`super::conditions::ConditionKind::Format`] evaluating to
//! [`Truth::Neutral`], and it stays true.
//! This module answers the other question, about the value the place carries.
//!
//! # What an unregistered number does
//!
//! [`evaluate`] returns [`FormatVerdict::Unregistered`], which the validator
//! reads as `Unknown`: it permits and never requires, the same direction every
//! unreadable condition takes here. The number is not lost, because
//! `cargo xtask validate-profiles` refuses an unregistered `[9xx]` cited by a
//! binding place at **import** time — loudly, once, against a ratchet — rather
//! than silently on every message.

use super::conditions::{ConditionKind, Expr, Truth};

/// What a Formatbedingung says about one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatVerdict {
    /// The value satisfies the condition.
    Holds,
    /// It does not, and this says how.
    Violated(String),
    /// No evaluator is registered for this number.
    Unregistered,
}

impl From<&FormatVerdict> for Truth {
    fn from(v: &FormatVerdict) -> Self {
        match v {
            FormatVerdict::Holds => Truth::True,
            FormatVerdict::Violated(_) => Truth::False,
            FormatVerdict::Unregistered => Truth::Unknown,
        }
    }
}

/// Evaluate Formatbedingung `id` against `value`.
///
/// `value` is the wire value of the data element the condition is attached to,
/// already unescaped. An empty value never reaches here: whether a place may be
/// empty is a Voraussetzung's question, not a Formatbedingung's.
#[must_use]
#[expect(
    clippy::match_same_arms,
    reason = "two Bedingungen with the same bound are still two Bedingungen: \
              BDEW may restate either one, and merging the arms would hide that"
)]
pub fn evaluate(id: &str, value: &str) -> FormatVerdict {
    match id {
        // ── Numeric bounds ────────────────────────────────────────────────
        "902" => at_least(value, "0"),
        "903" => exactly(value, "1"),
        "908" => at_least(value, "1"),
        // „Mögliche Werte: 0 bis n" — the same bound as [902] and a different
        // Bedingung; the AHB cites them at different places and BDEW may
        // restate either one, so they stay separate arms.
        "909" => at_least(value, "0"),
        // „Möglicher Wert: < 0 oder ≥ 0" admits every number and no
        // non-number, so it is the numeric assertion on its own.
        "910" => numeric(value).map_or_else(FormatVerdict::Violated, |_| FormatVerdict::Holds),
        "913" => between(value, "1", "99999"),
        "914" => greater_than(value, "0"),
        "915" => not_exactly(value, "1"),
        "926" => exactly(value, "0"),
        "927" => exactly(value, "-1"),
        "929" => exactly(value, "1000"),
        "938" => at_most(value, "10"),
        "955" => less_than(value, "100"),
        "963" => at_most(value, "100"),
        "968" => at_most(value, "0"),
        "969" => at_most(value, "1"),

        // ── Nachkommastellen ──────────────────────────────────────────────
        "906" => max_fraction_digits(value, 3),
        "907" => max_fraction_digits(value, 4),
        "912" => max_fraction_digits(value, 6),
        "925" => max_fraction_digits(value, 5),
        "930" => max_fraction_digits(value, 2),
        "937" => max_fraction_digits(value, 0),
        "946" => max_fraction_digits(value, 11),

        // ── Vorkommastellen ───────────────────────────────────────────────
        "917" => max_integer_digits(value, 4),
        "962" => max_integer_digits(value, 6),

        // ── Artikelnummern (DE 7140) ──────────────────────────────────────
        // The Bedingung prints the shape in the MIG's own representation
        // notation, where `n<k>` is k digits: `[942] n1-n2-n1-n3` is the
        // Artikel-ID `1-01-1-001`, `[943] n1-n2-n1` the Gruppenartikel-ID
        // `1-01-1` and `[959] n13-n2` the Artikelnummer `9991000000044-01`,
        // all three printed that way in Codeliste der Artikelnummern und
        // Artikel-ID 5.6.
        "942" => digit_groups(value, &[1, 2, 1, 3]),
        "943" => digit_groups(value, &[1, 2, 1]),
        "948" => digit_groups(value, &[1, 2, 1, 8, 2]),
        "949" => digit_groups(value, &[1, 2, 1, 8, 2, 1]),
        "957" => digit_groups(value, &[1, 2, 1, 8]),
        "959" => digit_groups(value, &[13, 2]),

        // ── Length ────────────────────────────────────────────────────────
        "904" => exact_length(value, 16),
        "905" => max_length(value, 3),

        // ── Zeitpunkte inside a `303` value ───────────────────────────────
        // Allgemeine Festlegungen 6.1d Kap. 3.8 defines [UB1]–[UB3] as
        // expressions over these four and [931]; they are the HHMM a day
        // boundary takes in UTC once gesetzliche deutsche Zeit is applied.
        "932" => dtm_303_field(value, 8, "2200", "HHMM"),
        "933" => dtm_303_field(value, 8, "2300", "HHMM"),
        "934" => dtm_303_field(value, 8, "0400", "HHMM"),
        "935" => dtm_303_field(value, 8, "0500", "HHMM"),
        // Kap. 3.8, transcribed from the published expressions:
        //   [UB1] ([931] ∧ [932] [490]) ⊻ ([931] ∧ [933] [491])
        //   [UB2] ([931] ∧ [934] [490]) ⊻ ([931] ∧ [935] [491])
        //   [UB3] the four combinations of those, selected by the recipient's
        //         Sparte — which a value does not carry, so both are admitted.
        // [490]/[491] are „der Zeitpunkt liegt im MESZ/MEZ-Zeitraum", which is
        // the value's own question and is answered by `is_mesz`.
        "UB1" => day_boundary(value, "2200", "2300", "des Tages"),
        "UB2" => day_boundary(value, "0400", "0500", "des Gas-Tages 06:00 Uhr"),
        "UB3" => match day_boundary(value, "2200", "2300", "") {
            FormatVerdict::Holds => FormatVerdict::Holds,
            _ => day_boundary(
                value,
                "0400",
                "0500",
                "des Tages (Strom) oder des Gas-Tages",
            ),
        },

        // ── Fixed shapes ──────────────────────────────────────────────────
        "918" => unoc_upper(value),
        "931" => zzz_utc(value),
        "939" => email_shape(value),
        "940" => phone_shape(value),
        "947" => dtm_303_field(value, 4, "12312300", "MMDDHHMM"),
        "964" => hhmm(value, "0000", true),
        "965" => hhmm(value, "2359", false),

        // ── Identifiers ───────────────────────────────────────────────────
        // BDEW Anwendungshilfe „Identifikatoren in der Marktkommunikation"
        // v1.2, which `rubo4e::identifiers` implements including the § 8.2
        // check-digit procedure. Delegating rather than restating it is what
        // keeps one scheme from being spelled twice.
        "922" => id_shape::<rubo4e::identifiers::TrId>(value, "a TR-ID"),
        "950" => id_shape::<rubo4e::identifiers::MaloId>(value, "a Marktlokations-ID"),
        "951" => {
            id_shape::<rubo4e::identifiers::Zaehlpunktbezeichnung>(value, "a Zählpunktbezeichnung")
        }
        "953" => {
            if matches!(
                id_shape::<rubo4e::identifiers::MaloId>(value, ""),
                FormatVerdict::Holds
            ) || matches!(
                id_shape::<rubo4e::identifiers::Zaehlpunktbezeichnung>(value, ""),
                FormatVerdict::Holds
            ) {
                FormatVerdict::Holds
            } else {
                FormatVerdict::Violated(
                    "is neither a Marktlokations-ID nor a Zählpunktbezeichnung".into(),
                )
            }
        }
        "960" => id_shape::<rubo4e::identifiers::NeloId>(value, "a Netzlokations-ID"),
        "961" => id_shape::<rubo4e::identifiers::SrId>(value, "an SR-ID"),

        _ => FormatVerdict::Unregistered,
    }
}

/// Every Formatbedingung [`evaluate`] answers.
///
/// `cargo xtask validate-profiles` reads this to refuse an unregistered number
/// cited by a binding place, so it is the list the ratchet runs down. It is
/// asserted against [`evaluate`] itself rather than written beside it.
pub const REGISTERED: &[&str] = &[
    "902", "903", "904", "905", "906", "907", "908", "909", "910", "912", "913", "914", "915",
    "917", "918", "922", "925", "926", "927", "929", "930", "931", "932", "933", "934", "935",
    "937", "938", "939", "940", "942", "943", "946", "947", "948", "949", "950", "951", "953",
    "955", "957", "959", "960", "961", "962", "963", "964", "965", "968", "969", "UB1", "UB2",
    "UB3",
];

/// Whether a number has an evaluator.
#[must_use]
pub fn is_registered(id: &str) -> bool {
    evaluate(id, "") != FormatVerdict::Unregistered
}

/// A value satisfying `id`, for a skeleton or a fixture.
///
/// The generator's own tables are keyed on the data element, so they cannot
/// know that this `LOC` wants a Zählpunktbezeichnung where the next one wants a
/// Marktlokations-ID — only the column knows. This is how it asks.
///
/// Every identifier here is the example its own specification prints, so
/// nothing that looks like a real assignment is minted: `41373559241` is the
/// worked example in the BDEW Anwendungshilfe „Identifikatoren in der
/// Marktkommunikation" and the `E`/`C`/`D` codes are `rubo4e`'s documented ones.
#[must_use]
#[expect(
    clippy::match_same_arms,
    reason = "two conditions that happen to share an example are still two \
              conditions; merging the arms would tie them together"
)]
pub fn example(id: &str) -> Option<&'static str> {
    Some(match id {
        "902" | "909" | "910" | "926" | "968" => "0",
        // HHMM, so four digits and not the bare bound.
        "964" | "965" => "0000",
        "903" | "905" | "906" | "907" | "908" | "912" | "913" | "914" | "925" | "930" | "937"
        | "938" | "946" | "955" | "962" | "963" | "969" | "917" => "1",
        "904" => "1234567890123456",
        "915" => "2",
        "918" => "A",
        "922" => "D0000000010",
        "927" => "-1",
        "929" => "1000",
        "931" => "202610010000+00",
        // 30.09.2026 22:00 UTC is 01.10.2026 00:00 MESZ; 04:00 UTC the same
        // day is 06:00 MESZ, the start of the Gas-Tag.
        "932" | "UB1" | "UB3" => "202609302200+00",
        "933" => "202612312300+00",
        "934" | "UB2" => "202609300400+00",
        "935" => "202612310500+00",
        "939" => "info@example.de",
        "940" => "+4915112345678",
        "942" => "1-01-1-001",
        "943" => "1-01-1",
        "947" => "202612312300+00",
        "948" => "1-01-1-00000001-01",
        "949" => "1-01-1-00000001-01-1",
        "957" => "1-01-1-00000001",
        "959" => "9991000000044-01",
        "950" | "953" => "41373559241",
        "951" => "DE0000000000000000000000000000042",
        "960" => "E0000000019",
        "961" => "C0000000011",
        _ => return None,
    })
}

/// Read an operand expression as a statement about the *value*.
///
/// This is not [`Expr::eval`], and the difference is the unit element. In the
/// presence question a Formatbedingung is `Neutral` because it never decides
/// whether a place appears; here the roles swap, and it is the Voraussetzungen
/// and Hinweise that decide nothing — so they read as `True`, the operand that
/// imposes no format constraint of its own. Keeping `Neutral` instead would
/// erase a branch: `X ([931] [13] ∧ [495]) ⊻ ([495] ∧ [515])` offers a `303`
/// Zeitpunkt or a plain date, and the second alternative — all Hinweis — would
/// vanish and leave the first one's failure standing alone.
///
/// `⊻` is read as `∨`. Allgemeine Festlegungen 6.1d Kap. 6.4.6 puts it
/// „zwischen zwei sich ausschließenden Formatbedingungen", which is a statement
/// about the conditions and not about the value: no value satisfies two
/// mutually exclusive formats, so „exactly one" and „at least one" agree
/// wherever the AHB uses it as documented, and where they do not, `∨` is the
/// reading that permits.
///
/// An unregistered number is `Unknown`, which propagates and never refuses.
#[must_use]
pub fn truth(expr: &Expr, value: &str) -> Truth {
    fold(expr, value, &mut Vec::new())
}

/// The Formatbedingungen `value` fails in `expr`, empty unless [`truth`] is
/// `False`.
///
/// Reporting only on `False` is what makes a branch the value failed silent
/// while another branch carries it: in `X [951] ∨ [950]` a Marktlokations-ID
/// fails `[951]` and the place is still correct.
#[must_use]
pub fn violations(expr: &Expr, value: &str) -> Vec<(String, String)> {
    let mut failed = Vec::new();
    if fold(expr, value, &mut failed) == Truth::False {
        failed
    } else {
        Vec::new()
    }
}

fn fold(expr: &Expr, value: &str, failed: &mut Vec<(String, String)>) -> Truth {
    match expr {
        Expr::Cond(id) => {
            if !matches!(
                ConditionKind::of(id),
                ConditionKind::Format | ConditionKind::Zeitpunkt
            ) {
                return Truth::True;
            }
            let v = evaluate(id, value);
            if let FormatVerdict::Violated(why) = &v {
                failed.push((id.clone(), why.clone()));
            }
            Truth::from(&v)
        }
        Expr::Then(v) | Expr::And(v) => {
            let vals: Vec<Truth> = v.iter().map(|e| fold(e, value, failed)).collect();
            if vals.contains(&Truth::False) {
                Truth::False
            } else if vals.contains(&Truth::Unknown) {
                Truth::Unknown
            } else {
                Truth::True
            }
        }
        Expr::Or(v) | Expr::Xor(v) => {
            let vals: Vec<Truth> = v.iter().map(|e| fold(e, value, failed)).collect();
            if vals.contains(&Truth::True) {
                Truth::True
            } else if vals.contains(&Truth::Unknown) {
                Truth::Unknown
            } else {
                Truth::False
            }
        }
    }
}

// ── Identifier delegation ─────────────────────────────────────────────────────

/// The shared constructor of every `rubo4e` identifier.
trait BdewId: Sized {
    fn parse(s: &str) -> Result<Self, rubo4e::prelude::IdentifierError>;
}

macro_rules! bdew_id {
    ($($t:ty),* $(,)?) => {$(
        impl BdewId for $t {
            fn parse(s: &str) -> Result<Self, rubo4e::prelude::IdentifierError> {
                Self::new(s)
            }
        }
    )*};
}

bdew_id!(
    rubo4e::identifiers::MaloId,
    rubo4e::identifiers::NeloId,
    rubo4e::identifiers::SrId,
    rubo4e::identifiers::TrId,
    rubo4e::identifiers::Zaehlpunktbezeichnung,
);

fn id_shape<T: BdewId>(value: &str, what: &str) -> FormatVerdict {
    match T::parse(value) {
        Ok(_) => FormatVerdict::Holds,
        Err(e) => FormatVerdict::Violated(format!("is not {what}: {e}")),
    }
}

// ── Decimals ──────────────────────────────────────────────────────────────────

/// A wire number, kept as digits so a comparison is exact.
///
/// The decimal mark is `.`: UNA is not consulted because every EDI@Energy
/// interchange uses the default service string advice, which
/// [`crate::interchange`] pins.
#[derive(Debug, PartialEq, Eq)]
struct Dec {
    negative: bool,
    /// Integer digits, leading zeros stripped; empty means zero.
    int: String,
    /// Fraction digits, trailing zeros stripped.
    frac: String,
}

impl Dec {
    fn parse(s: &str) -> Option<Self> {
        let (negative, body) = match s.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        if body.is_empty() {
            return None;
        }
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        let int = int.trim_start_matches('0').to_owned();
        let frac = frac.trim_end_matches('0').to_owned();
        // `-0` and `0` are the same number, and treating them otherwise would
        // make `[926] = 0` depend on how the sender wrote zero.
        let negative = negative && !(int.is_empty() && frac.is_empty());
        Some(Self {
            negative,
            int,
            frac,
        })
    }

    fn is_integer(&self) -> bool {
        self.frac.is_empty()
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self.negative, other.negative) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.cmp(&other.int))
            .then_with(|| {
                // Pad to a common width so `.5` outranks `.45`.
                let w = self.frac.len().max(other.frac.len());
                let pad = |f: &str| format!("{f:0<w$}");
                pad(&self.frac).cmp(&pad(&other.frac))
            });
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn numeric(value: &str) -> Result<Dec, String> {
    Dec::parse(value).ok_or_else(|| "is not a number".to_owned())
}

/// The bound as the registry writes it.
///
/// A literal that does not parse is a typo here rather than a fault in the
/// message, and the verdict it produces is [`FormatVerdict::Unregistered`] —
/// which permits the value and, because [`is_registered`] reads the same
/// answer, makes `cargo xtask validate-profiles` refuse every profile citing
/// the number. A mistyped bound is therefore a loud CI failure and never a
/// wrong verdict about somebody's message.
fn bound(literal: &str) -> Option<Dec> {
    Dec::parse(literal)
}

fn compare(
    value: &str,
    literal: &str,
    ok: impl Fn(std::cmp::Ordering) -> bool,
    say: &str,
) -> FormatVerdict {
    let Some(limit) = bound(literal) else {
        return FormatVerdict::Unregistered;
    };
    match numeric(value) {
        Err(e) => FormatVerdict::Violated(e),
        Ok(d) if ok(d.cmp(&limit)) => FormatVerdict::Holds,
        Ok(_) => FormatVerdict::Violated(format!("is {value}, not {say}")),
    }
}

fn at_least(value: &str, literal: &str) -> FormatVerdict {
    compare(
        value,
        literal,
        std::cmp::Ordering::is_ge,
        &format!("≥ {literal}"),
    )
}

fn at_most(value: &str, literal: &str) -> FormatVerdict {
    compare(
        value,
        literal,
        std::cmp::Ordering::is_le,
        &format!("≤ {literal}"),
    )
}

fn greater_than(value: &str, literal: &str) -> FormatVerdict {
    compare(
        value,
        literal,
        std::cmp::Ordering::is_gt,
        &format!("> {literal}"),
    )
}

fn less_than(value: &str, literal: &str) -> FormatVerdict {
    compare(
        value,
        literal,
        std::cmp::Ordering::is_lt,
        &format!("< {literal}"),
    )
}

fn exactly(value: &str, literal: &str) -> FormatVerdict {
    compare(value, literal, std::cmp::Ordering::is_eq, literal)
}

fn not_exactly(value: &str, literal: &str) -> FormatVerdict {
    let Some(limit) = bound(literal) else {
        return FormatVerdict::Unregistered;
    };
    match numeric(value) {
        Err(e) => FormatVerdict::Violated(e),
        Ok(d) if d == limit => FormatVerdict::Violated(format!("is {literal}")),
        Ok(_) => FormatVerdict::Holds,
    }
}

fn between(value: &str, low: &str, high: &str) -> FormatVerdict {
    let (Some(lo), Some(hi)) = (bound(low), bound(high)) else {
        return FormatVerdict::Unregistered;
    };
    match numeric(value) {
        Err(e) => FormatVerdict::Violated(e),
        Ok(d) if d < lo || d > hi => {
            FormatVerdict::Violated(format!("is {value}, outside {low}–{high}"))
        }
        Ok(d) if !d.is_integer() => {
            FormatVerdict::Violated(format!("is {value}, which is not a whole number"))
        }
        Ok(_) => FormatVerdict::Holds,
    }
}

fn max_fraction_digits(value: &str, max: usize) -> FormatVerdict {
    let Some(d) = Dec::parse(value) else {
        return FormatVerdict::Violated("is not a number".into());
    };
    // Kap. 2.18.3 has the sender truncate rather than round, so a trailing
    // zero is padding and not a Nachkommastelle — `Dec::parse` drops it.
    if d.frac.len() <= max {
        FormatVerdict::Holds
    } else if max == 0 {
        FormatVerdict::Violated(format!("is {value}, which has a Nachkommastelle"))
    } else {
        FormatVerdict::Violated(format!(
            "is {value}, which has {} Nachkommastellen, not at most {max}",
            d.frac.len()
        ))
    }
}

fn max_integer_digits(value: &str, max: usize) -> FormatVerdict {
    let Some(d) = Dec::parse(value) else {
        return FormatVerdict::Violated("is not a number".into());
    };
    if d.int.len() <= max {
        FormatVerdict::Holds
    } else {
        FormatVerdict::Violated(format!(
            "is {value}, which has {} Vorkommastellen, not at most {max}",
            d.int.len()
        ))
    }
}

// ── Fixed shapes ──────────────────────────────────────────────────────────────

fn exact_length(value: &str, len: usize) -> FormatVerdict {
    let n = value.chars().count();
    if n == len {
        FormatVerdict::Holds
    } else {
        FormatVerdict::Violated(format!("is {n} characters long, not exactly {len}"))
    }
}

fn max_length(value: &str, len: usize) -> FormatVerdict {
    let n = value.chars().count();
    if n <= len {
        FormatVerdict::Holds
    } else {
        FormatVerdict::Violated(format!("is {n} characters long, not at most {len}"))
    }
}

/// Whether the `CCYYMMDDHHMM` of a `303` Zeitpunkt falls in gesetzliche
/// deutsche Sommerzeit.
///
/// Allgemeine Festlegungen 6.1d Kap. 3.5/3.6 publish the two periods row by
/// row in UTC, and every row is the last Sunday of March and of October at
/// 01:00 UTC — Richtlinie 2000/84/EG Art. 2 and 3. The rule is computed rather
/// than transcribed because the published table stops at 2032, and a table
/// that runs out would answer „Winterzeit" for every later Zeitpunkt instead
/// of saying it does not know.
fn is_mesz(value: &str) -> Option<bool> {
    let year: i32 = value.get(..4)?.parse().ok()?;
    let month: u8 = value.get(4..6)?.parse().ok()?;
    let day: u8 = value.get(6..8)?.parse().ok()?;
    let hour: u8 = value.get(8..10)?.parse().ok()?;
    let minute: u8 = value.get(10..12)?.parse().ok()?;
    let date =
        time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    let at = date.with_hms(hour, minute, 0).ok()?;
    let switch = |month: time::Month, last: u8| -> Option<time::PrimitiveDateTime> {
        let mut d = time::Date::from_calendar_date(year, month, last).ok()?;
        while d.weekday() != time::Weekday::Sunday {
            d = d.previous_day()?;
        }
        d.with_hms(1, 0, 0).ok()
    };
    let from = switch(time::Month::March, 31)?;
    let until = switch(time::Month::October, 31)?;
    Some(at >= from && at < until)
}

/// `[UB1]`/`[UB2]`/`[UB3]` — a Zeitpunkt on a day boundary in gesetzliche
/// deutscher Zeit, written in UTC.
fn day_boundary(value: &str, summer: &str, winter: &str, what: &str) -> FormatVerdict {
    // [931], which every one of the three conjoins.
    if !value.ends_with("+00") {
        return FormatVerdict::Violated(format!("is {value:?}, which does not end on \"+00\""));
    }
    let Some(mesz) = is_mesz(value) else {
        return FormatVerdict::Violated(format!("is {value:?}, not a CCYYMMDDHHMMZZZ Zeitpunkt"));
    };
    let want = if mesz { summer } else { winter };
    let got = &value[8..12];
    if got == want {
        return FormatVerdict::Holds;
    }
    let season = if mesz { "MESZ" } else { "MEZ" };
    FormatVerdict::Violated(format!(
        "has HHMM {got}; in {season} the Beginn-/Ende-Zeitpunkt {what} is {want}"
    ))
}

/// Dash-separated groups of digits, as `n1-n2-n1-n3`.
fn digit_groups(value: &str, widths: &[usize]) -> FormatVerdict {
    let shape = || {
        widths
            .iter()
            .map(|w| format!("n{w}"))
            .collect::<Vec<_>>()
            .join("-")
    };
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != widths.len() {
        return FormatVerdict::Violated(format!(
            "is {value:?}, which has {} dash-separated groups, not the {} of {}",
            parts.len(),
            widths.len(),
            shape()
        ));
    }
    for (part, want) in parts.iter().zip(widths) {
        if part.len() != *want || !part.bytes().all(|b| b.is_ascii_digit()) {
            return FormatVerdict::Violated(format!("is {value:?}, not {}", shape()));
        }
    }
    FormatVerdict::Holds
}

/// A fixed run of characters inside a `303` Zeitpunkt.
///
/// DE 2380 under DE 2379 = `303` is `CCYYMMDDHHMMZZZ`, and a Formatbedingung
/// naming `MMDDHHMM` or `HHMM` constrains that slice of it rather than the
/// whole value — `[947] Format: MMDDHHMM = 12312300` is 31 December 23:00 UTC,
/// which is the turn of the year in gesetzliche deutsche Zeit.
fn dtm_303_field(value: &str, at: usize, want: &str, field: &str) -> FormatVerdict {
    if value.len() != 15 || !value.as_bytes()[..12].iter().all(u8::is_ascii_digit) {
        return FormatVerdict::Violated(format!("is {value:?}, not a CCYYMMDDHHMMZZZ Zeitpunkt"));
    }
    let got = &value[at..at + want.len()];
    if got == want {
        FormatVerdict::Holds
    } else {
        FormatVerdict::Violated(format!("has {field} {got}, not {want}"))
    }
}

/// `[918]` — UNOC with the letters restricted to upper case.
///
/// UNOC (ISO 9735 Level C) is Latin-1. The condition keeps its whole
/// repertoire and removes the lower-case letters.
fn unoc_upper(value: &str) -> FormatVerdict {
    if let Some(c) = value.chars().find(|c| c.is_lowercase()) {
        return FormatVerdict::Violated(format!("carries the lower-case letter {c:?}"));
    }
    match value.chars().find(|c| !matches!(*c, ' '..='~' | ' '..='ÿ')) {
        Some(c) => FormatVerdict::Violated(format!("carries {c:?}, which UNOC does not define")),
        None => FormatVerdict::Holds,
    }
}

/// `[931]` „ZZZ = +00" — the UTC offset of a DTM 303 Zeitpunkt.
///
/// DE 2380 in format 303 is `CCYYMMDDHHMMZZZ`, where `ZZZ` is the offset from
/// UTC. `+00` is the market's one admitted value, which is what makes every
/// EDI@Energy Zeitpunkt a UTC instant.
fn zzz_utc(value: &str) -> FormatVerdict {
    if value.ends_with("+00") {
        FormatVerdict::Holds
    } else {
        FormatVerdict::Violated(format!("is {value:?}, which does not end on \"+00\""))
    }
}

/// `[939]` „Die Zeichenkette muss die Zeichen @ und . enthalten".
///
/// Deliberately the AHB's own test and not an address grammar: a stricter
/// screen here would refuse a deliverable address the AHB admits.
fn email_shape(value: &str) -> FormatVerdict {
    match (value.contains('@'), value.contains('.')) {
        (true, true) => FormatVerdict::Holds,
        (false, true) => FormatVerdict::Violated("does not contain \"@\"".into()),
        (true, false) => FormatVerdict::Violated("does not contain \".\"".into()),
        (false, false) => FormatVerdict::Violated("contains neither \"@\" nor \".\"".into()),
    }
}

/// `[940]` „muss mit dem Zeichen + beginnen und danach dürfen nur noch Ziffern
/// folgen" — so a national form, a space or a dash is a violation.
fn phone_shape(value: &str) -> FormatVerdict {
    let Some(digits) = value.strip_prefix('+') else {
        return FormatVerdict::Violated("does not begin with \"+\"".into());
    };
    if digits.is_empty() {
        return FormatVerdict::Violated("is \"+\" with no digits".into());
    }
    match digits.chars().find(|c| !c.is_ascii_digit()) {
        Some(c) => FormatVerdict::Violated(format!("carries {c:?} after the \"+\"")),
        None => FormatVerdict::Holds,
    }
}

/// `[964]`/`[965]` — `HHMM` against a bound.
fn hhmm(value: &str, limit: &str, at_least_limit: bool) -> FormatVerdict {
    if value.len() != 4 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return FormatVerdict::Violated(format!("is {value:?}, not four digits HHMM"));
    }
    let ok = if at_least_limit {
        value >= limit
    } else {
        value <= limit
    };
    if ok {
        FormatVerdict::Holds
    } else {
        let rel = if at_least_limit { "≥" } else { "≤" };
        FormatVerdict::Violated(format!("is {value}, not {rel} {limit}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds(id: &str, v: &str) {
        assert_eq!(evaluate(id, v), FormatVerdict::Holds, "[{id}] on {v:?}");
    }

    fn violated(id: &str, v: &str) {
        assert!(
            matches!(evaluate(id, v), FormatVerdict::Violated(_)),
            "[{id}] should refuse {v:?}, got {:?}",
            evaluate(id, v)
        );
    }

    #[test]
    fn registered_is_the_list_evaluate_answers() {
        // Not a second enumeration: every number 901–999 and every `UB` the
        // Allgemeine Festlegungen name are asked, and the list has to be
        // exactly the ones that answer.
        let answered: Vec<String> = (901..=999)
            .map(|n| n.to_string())
            .chain((1..=9).map(|n| format!("UB{n}")))
            .filter(|id| evaluate(id, "") != FormatVerdict::Unregistered)
            .collect();
        assert_eq!(answered, REGISTERED);
    }

    #[test]
    fn an_unregistered_number_is_unregistered_and_not_a_violation() {
        // `[952]` Gerätenummer nach DIN 43863-5 has no evaluator: the value
        // must permit, never refuse.
        assert_eq!(evaluate("952", "anything"), FormatVerdict::Unregistered);
        assert_eq!(Truth::from(&FormatVerdict::Unregistered), Truth::Unknown);
        assert!(!is_registered("952"));
        assert!(is_registered("931"));
    }

    #[test]
    fn every_registered_number_has_an_example_that_satisfies_it() {
        // Not a second enumeration: `example` is checked against `evaluate`,
        // so a placeholder that stops satisfying its own condition is a test
        // failure rather than a skeleton the Prüfschablone refuses.
        for id in REGISTERED {
            let Some(v) = example(id) else {
                panic!("[{id}] is registered but has no example");
            };
            assert_eq!(
                evaluate(id, v),
                FormatVerdict::Holds,
                "[{id}]'s example {v:?} does not satisfy it"
            );
        }
        assert_eq!(
            example("952"),
            None,
            "an unregistered number has no example"
        );
    }

    #[test]
    fn zzz_is_the_utc_offset() {
        holds("931", "202510010000+00");
        violated("931", "202510010000+01");
        violated("931", "202510010000");
    }

    #[test]
    fn nachkommastellen_count_significant_digits() {
        holds("937", "42");
        // Kap. 2.18.3 truncates rather than rounds, so `42.00` carries no
        // Nachkommastelle — refusing it would refuse a sender obeying the MIG.
        holds("937", "42.00");
        violated("937", "42.5");
        holds("930", "42.25");
        violated("930", "42.255");
        holds("906", "0.125");
        violated("906", "0.1255");
    }

    #[test]
    fn numeric_bounds_compare_exactly() {
        holds("914", "0.0000000001");
        violated("914", "0");
        violated("914", "-0.5");
        holds("902", "0");
        holds("902", "-0");
        violated("902", "-0.0000000001");
        holds("903", "1");
        holds("903", "1.0");
        violated("903", "2");
        violated("915", "1");
        holds("915", "2");
        holds("938", "10");
        violated("938", "10.5");
        violated("955", "100");
        holds("955", "99.999");
    }

    #[test]
    fn a_bound_does_not_lose_precision_on_a_long_number() {
        // The digits below exceed f64's 53 bits; a float comparison would
        // call both of these 100.
        violated("955", "100.0000000000000000001");
        holds("963", "99.9999999999999999999");
    }

    #[test]
    fn a_whole_number_range_refuses_a_fraction() {
        holds("913", "99999");
        violated("913", "100000");
        violated("913", "0");
        violated("913", "1.5");
    }

    #[test]
    fn vorkommastellen_and_length() {
        holds("962", "123456.789");
        violated("962", "1234567");
        holds("904", "1234567890123456");
        violated("904", "123456789012345");
        holds("905", "abc");
        violated("905", "abcd");
    }

    #[test]
    fn the_string_shapes_are_the_ahbs_own_tests() {
        holds("939", "a@b.de");
        violated("939", "ab.de");
        violated("939", "a@bde");
        holds("940", "+4915112345678");
        violated("940", "015112345678");
        violated("940", "+49 151 12345678");
        violated("940", "+");
        holds("918", "ABC-123");
        violated("918", "Abc");
        // [947] names MMDDHHMM inside a CCYYMMDDHHMMZZZ Zeitpunkt, so it is
        // the turn of the year and not a standalone eight-digit value.
        holds("947", "202612312300+00");
        violated("947", "202610010000+00");
        violated("947", "12312300");
        holds("964", "0000");
        holds("965", "2359");
        violated("965", "2400");
        violated("965", "abc");
    }

    /// Rows copied from Allgemeine Festlegungen 6.1d Kap. 3.5, „Darstellung in
    /// UTC" — the computed rule has to reproduce the published table, and the
    /// table is what the rule would otherwise be trusted to replace.
    #[test]
    fn the_season_rule_reproduces_the_published_table() {
        // (start of MESZ, start of MEZ) per Kap. 3.5.
        for (year, mesz_from, mez_from) in [
            (2024, "20240331", "20241027"),
            (2025, "20250330", "20251026"),
            (2026, "20260329", "20261025"),
            (2027, "20270328", "20271031"),
            (2032, "20320328", "20321031"),
        ] {
            assert_eq!(
                is_mesz(&format!("{mesz_from}0100")),
                Some(true),
                "{year}: MESZ begins at 01:00 UTC"
            );
            assert_eq!(
                is_mesz(&format!("{mesz_from}0059")),
                Some(false),
                "{year}: the minute before is still MEZ"
            );
            assert_eq!(
                is_mesz(&format!("{mez_from}0100")),
                Some(false),
                "{year}: MESZ ends at 01:00 UTC"
            );
            assert_eq!(
                is_mesz(&format!("{mez_from}0059")),
                Some(true),
                "{year}: the minute before is still MESZ"
            );
        }
        assert_eq!(is_mesz("not-a-zeitpunkt"), None);
    }

    #[test]
    fn a_day_boundary_is_the_hhmm_its_season_admits() {
        // 30.09.2026 22:00 UTC is 01.10.2026 00:00 MESZ.
        holds("UB1", "202609302200+00");
        violated("UB1", "202609302300+00");
        // 31.12.2026 23:00 UTC is 01.01.2027 00:00 MEZ.
        holds("UB1", "202612312300+00");
        violated("UB1", "202612312200+00");
        // The Gas-Tag starts at 06:00 gesetzlicher deutscher Zeit.
        holds("UB2", "202609300400+00");
        holds("UB2", "202612310500+00");
        violated("UB2", "202609302200+00");
        // [UB3] cannot see the recipient's Sparte, so it admits both.
        holds("UB3", "202609302200+00");
        holds("UB3", "202609300400+00");
        violated("UB3", "202609301200+00");
        // [931] is conjoined into all three.
        violated("UB1", "202609302200+01");
        violated("UB1", "20260930");
    }

    #[test]
    fn an_artikelnummer_is_read_as_groups_of_digits() {
        // Values printed in Codeliste der Artikelnummern und Artikel-ID 5.6.
        holds("942", "1-01-1-001");
        holds("943", "1-01-1");
        holds("959", "9991000000044-01");
        violated("942", "1-01-1");
        violated("942", "1-1-1-001");
        violated("942", "1-01-1-00A");
        violated("959", "9991000000044");
    }

    #[test]
    fn an_identifier_is_checked_by_its_own_scheme() {
        // `41373559241` is the worked example printed in the BDEW
        // Anwendungshilfe; `…242` is the same body with a wrong check digit,
        // which is the half a length screen cannot see.
        holds("950", "41373559241");
        holds("953", "41373559241");
        violated("950", "41373559242");
        violated("950", "4137355924");
        violated("950", "not-an-id");
        holds("951", "DE0000000000000000000000000000042");
        holds("953", "DE0000000000000000000000000000042");
        violated("951", "DE123");
        // A NeLo-ID is the 11-character form, not the 10-character body.
        violated("960", "E000000001");
        holds("960", "E0000000019");
        violated("953", "nonsense");
    }
}
