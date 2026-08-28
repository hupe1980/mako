//! Contract-law rules of the German retail energy market, as pure functions.
//!
//! Every deadline `vertragd` enforces comes from a statute, and the statute
//! differs by *which* contract and *which* customer. Encoding that here — away
//! from HTTP, SQL and the clock — is what makes the rules testable and keeps a
//! handler from inventing a fourth notice period.
//!
//! # The rules, and where they come from
//!
//! | Rule | Source | Value |
//! |---|---|---|
//! | Kündigung Grundversorgung | § 20 Abs. 1 StromGVV / GasGVV | 2 Wochen, jederzeit |
//! | Kündigungsbestätigung | § 20 Abs. 2 GVV / § 41 Abs. 8 Nr. 2 EnWG | unverzüglich, Textform |
//! | Kündigung Sondervertrag | Vertrag, gedeckelt durch § 309 Nr. 9 BGB | ≤ 1 Monat für Verbraucher |
//! | Sonderkündigung Preisanpassung | § 41 Abs. 5 Satz 4 EnWG, § 5 Abs. 3 GVV | fristlos zum Wirksamwerden |
//! | Sonderkündigung Umzug | § 41b Abs. 5 EnWG | 6 Wochen |
//! | Preisänderungsanzeige Sondervertrag | § 41 Abs. 5 Satz 2 EnWG | 1 Monat (Haushaltskunde), sonst 2 Wochen |
//! | Preisänderungsanzeige Grundversorgung | § 5 Abs. 2 StromGVV / GasGVV | 6 Wochen, nur zum Monatsersten |
//! | Erstlaufzeit Verbrauchervertrag | § 309 Nr. 9 lit. a BGB | ≤ 24 Monate |
//! | Stillschweigende Verlängerung | § 309 Nr. 9 lit. b BGB | nur auf unbestimmte Zeit, ≤ 1 Monat kündbar |
//! | Ersatzversorgung | § 38 Abs. 4 EnWG | endet spätestens nach 3 Monaten |
//!
//! # What this module deliberately does not decide
//!
//! Whether a customer *is* a Haushaltskunde. § 3 Nr. 57 EnWG makes that a fact
//! about annual consumption (≤ 10 000 kWh also qualifies a commercial buyer),
//! not about a customer-type label, so it is stored per Kunde and passed in.

use serde::{Deserialize, Serialize};
use time::Date;

// ── Contract classification ───────────────────────────────────────────────────

/// Which supply regime a contract falls under — it decides every deadline below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Vertragsart {
    /// § 36 EnWG Grundversorgung — StromGVV / GasGVV apply in full.
    Grundversorgung,
    /// § 38 EnWG Ersatzversorgung — ends after three months at the latest.
    Ersatzversorgung,
    /// Any contract outside the Grundversorgung (§ 41b EnWG).
    Sondervertrag,
}

impl Vertragsart {
    /// Parse the stored column value; unknown text is a Sondervertrag, the
    /// regime with the *least* statutory privilege, so a typo cannot silently
    /// grant Grundversorgungs-Fristen.
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "GRUNDVERSORGUNG" => Self::Grundversorgung,
            "ERSATZVERSORGUNG" => Self::Ersatzversorgung,
            _ => Self::Sondervertrag,
        }
    }

    /// The column value.
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Grundversorgung => "GRUNDVERSORGUNG",
            Self::Ersatzversorgung => "ERSATZVERSORGUNG",
            Self::Sondervertrag => "SONDERVERTRAG",
        }
    }

    /// `true` for the two regimes the GVV governs.
    #[must_use]
    pub const fn ist_grundversorgung(self) -> bool {
        matches!(self, Self::Grundversorgung | Self::Ersatzversorgung)
    }
}

/// Why a contract is being terminated. The reason, not the contract, decides
/// the notice period — a Sonderkündigung overrides whatever the contract says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kuendigungsgrund {
    /// Ordinary termination on the contractual notice period.
    Ordentlich,
    /// § 41 Abs. 5 Satz 4 EnWG / § 5 Abs. 3 GVV — the supplier changed prices or
    /// terms, so the customer terminates without notice, effective the day the
    /// change would take effect.
    Preisanpassung,
    /// § 41b Abs. 5 EnWG — Haushaltskunde moving house: six weeks.
    Umzug,
    /// Termination declared by the new supplier on the customer's behalf in a
    /// Lieferantenwechsel. Same period as an ordinary one; it exists as its own
    /// reason because § 41 Abs. 8 Nr. 3 EnWG obliges the outgoing supplier to
    /// return an electronic Kündigungsbestätigung to that supplier as well.
    Lieferantenwechsel,
}

impl Kuendigungsgrund {
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "PREISANPASSUNG" => Self::Preisanpassung,
            "UMZUG" => Self::Umzug,
            "LIEFERANTENWECHSEL" => Self::Lieferantenwechsel,
            _ => Self::Ordentlich,
        }
    }

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Ordentlich => "ORDENTLICH",
            Self::Preisanpassung => "PREISANPASSUNG",
            Self::Umzug => "UMZUG",
            Self::Lieferantenwechsel => "LIEFERANTENWECHSEL",
        }
    }
}

// ── Kündigungsfristen ─────────────────────────────────────────────────────────

/// The earliest date a termination may take effect, with the rule that produced
/// it — the API returns both so a rejected Kündigung says *why*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Kuendigungsfrist {
    /// Earliest permissible `lieferende`, counted from the day notice arrives.
    pub fruehestens: Date,
    /// The statute the period comes from, for the response and the audit trail.
    pub rechtsgrundlage: &'static str,
    /// Human-readable period ("2 Wochen", "1 Monat", "fristlos").
    pub frist: String,
}

/// The earliest effective date for a termination received on `eingang`.
///
/// `vertragliche_frist_monate` is only consulted where the statute leaves the
/// period to the contract — it never lengthens a statutory Sonderkündigung and
/// never survives the § 309 Nr. 9 lit. c BGB cap for a consumer.
///
/// `preisanpassung_wirksam_zum` carries the date the announced price change
/// takes effect; a § 41 Abs. 5 Satz 4 Sonderkündigung ends the contract exactly
/// then. Without it the termination is treated as effective immediately, which
/// is the customer-favourable reading of a Sonderkündigung with no announced
/// change to attach to.
#[must_use]
pub fn kuendigungsfrist(
    eingang: Date,
    vertragsart: Vertragsart,
    haushaltskunde: bool,
    grund: Kuendigungsgrund,
    vertragliche_frist_monate: i32,
    preisanpassung_wirksam_zum: Option<Date>,
) -> Kuendigungsfrist {
    match grund {
        Kuendigungsgrund::Preisanpassung => Kuendigungsfrist {
            fruehestens: preisanpassung_wirksam_zum.unwrap_or(eingang),
            rechtsgrundlage: if vertragsart.ist_grundversorgung() {
                "§ 5 Abs. 3 StromGVV / GasGVV"
            } else {
                "§ 41 Abs. 5 Satz 4 EnWG"
            },
            frist: "fristlos zum Wirksamwerden der Änderung".to_owned(),
        },
        // § 41b Abs. 5 EnWG grants the six-week Umzugskündigung to
        // Haushaltskunden. A non-household customer keeps the ordinary period,
        // which is why this arm falls through rather than applying six weeks to
        // everyone who claims a move.
        Kuendigungsgrund::Umzug if haushaltskunde => Kuendigungsfrist {
            fruehestens: eingang + time::Duration::weeks(6),
            rechtsgrundlage: "§ 41b Abs. 5 EnWG",
            frist: "6 Wochen".to_owned(),
        },
        _ if vertragsart.ist_grundversorgung() => Kuendigungsfrist {
            fruehestens: eingang + time::Duration::weeks(2),
            rechtsgrundlage: "§ 20 Abs. 1 StromGVV / GasGVV",
            frist: "2 Wochen".to_owned(),
        },
        _ => {
            let monate =
                zulaessige_kuendigungsfrist_monate(haushaltskunde, vertragliche_frist_monate);
            Kuendigungsfrist {
                fruehestens: add_months(eingang, monate),
                rechtsgrundlage: if monate < vertragliche_frist_monate {
                    "§ 309 Nr. 9 lit. c BGB (vertragliche Frist gekürzt)"
                } else {
                    "vertragliche Kündigungsfrist"
                },
                frist: format!("{monate} Monat(e)"),
            }
        }
    }
}

/// The contractual notice period after the § 309 Nr. 9 lit. c BGB cap.
///
/// A consumer contract may not impose more than one month's notice, so a stored
/// three-month period is not enforced against a Verbraucher — it is capped
/// here rather than rejected at write time, because the clause is void, not the
/// contract.
#[must_use]
pub fn zulaessige_kuendigungsfrist_monate(haushaltskunde: bool, vertraglich: i32) -> i32 {
    let frist = vertraglich.max(0);
    if haushaltskunde { frist.min(1) } else { frist }
}

// ── Preisanpassung (§ 41 Abs. 5 EnWG / § 5 Abs. 2 GVV) ────────────────────────

/// The shape of a statutory notice period.
///
/// Weeks and months are **not** interchangeable with a day count. § 188 Abs. 2
/// BGB ends a month-denominated period on the day of the following month that
/// bears the same number as the start day — so „1 Monat" from 1 January ends on
/// 1 February (31 days), and from 1 February on 1 March (28). A flat 30 days is
/// one day *short* of the statute for every 31-day month, and two days longer
/// than it for February.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "einheit", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Frist {
    /// A day-denominated period. Weeks are exact multiples of 7 days, so they
    /// are expressed here.
    Tage {
        /// Number of days.
        tage: i64,
    },
    /// A month-denominated period under § 188 Abs. 2 BGB.
    Monate {
        /// Number of months.
        monate: u32,
    },
}

impl Frist {
    /// The earliest date on which a notice given on `ab` may take effect.
    ///
    /// § 188 Abs. 3 BGB: where the target month has no day of that number, the
    /// period ends on its last day — one month from 31 January is 28 (or 29)
    /// February.
    #[must_use]
    pub fn fruehestens_ab(self, ab: Date) -> Date {
        match self {
            Self::Tage { tage } => ab + time::Duration::days(tage),
            Self::Monate { monate } => add_monate(ab, monate),
        }
    }

    /// Whether a notice given on `ab` is early enough for `wirksamkeit`.
    #[must_use]
    pub fn gewahrt(self, ab: Date, wirksamkeit: Date) -> bool {
        wirksamkeit >= self.fruehestens_ab(ab)
    }
}

/// Add whole calendar months, clamping to the last day of the target month.
fn add_monate(from: Date, monate: u32) -> Date {
    let total = i32::from(u8::from(from.month())) - 1 + i32::try_from(monate).unwrap_or(0);
    let jahr = from.year() + total.div_euclid(12);
    let monat_index = u8::try_from(total.rem_euclid(12) + 1).unwrap_or(1);
    let monat = time::Month::try_from(monat_index).unwrap_or(time::Month::January);
    // § 188 Abs. 3 BGB — the 31st of a 30-day month is that month's last day.
    let letzter = time::util::days_in_month(monat, jahr);
    let tag = from.day().min(letzter);
    Date::from_calendar_date(jahr, monat, tag).unwrap_or(from)
}

/// How much notice a price change needs, and under which rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preisanpassungsregime {
    /// The statutory period, in the unit the statute uses.
    pub frist: Frist,
    pub rechtsgrundlage: &'static str,
    /// Human-readable period.
    pub bezeichnung: String,
    /// § 5 Abs. 2 GVV lets a Grundversorgungs-price change take effect only at
    /// the start of a month.
    pub nur_zum_monatsersten: bool,
}

impl Preisanpassungsregime {
    /// The earliest Wirksamkeit for a notice given on `ab`.
    ///
    /// For the Grundversorgung the § 5 Abs. 2 GVV Monatsersten rule applies on
    /// top: a change may only take effect at the start of a month, so the
    /// earliest date rolls forward to the next one.
    #[must_use]
    pub fn fruehestens_wirksam(&self, ab: Date) -> Date {
        let fruehestens = self.frist.fruehestens_ab(ab);
        if self.nur_zum_monatsersten && fruehestens.day() != 1 {
            add_monate(
                Date::from_calendar_date(fruehestens.year(), fruehestens.month(), 1)
                    .unwrap_or(fruehestens),
                1,
            )
        } else {
            fruehestens
        }
    }
}

/// The notice regime for a price change on this contract.
///
/// Three different periods live in two statutes, and picking the wrong one is
/// either a regulatory breach (too short) or a competitive handicap (too long):
///
/// - Grundversorgung: **6 weeks**, effective only at a month start
///   (§ 5 Abs. 2 StromGVV / GasGVV);
/// - Sondervertrag, Haushaltskunde: **1 month** (§ 41 Abs. 5 Satz 2 EnWG);
/// - Sondervertrag, sonstige Letztverbraucher: **2 weeks** (same sentence).
#[must_use]
pub fn preisanpassungsregime(
    vertragsart: Vertragsart,
    haushaltskunde: bool,
) -> Preisanpassungsregime {
    if vertragsart.ist_grundversorgung() {
        Preisanpassungsregime {
            frist: Frist::Tage { tage: 42 },
            rechtsgrundlage: "§ 5 Abs. 2 StromGVV / GasGVV",
            bezeichnung: "6 Wochen".to_owned(),
            nur_zum_monatsersten: true,
        }
    } else if haushaltskunde {
        Preisanpassungsregime {
            frist: Frist::Monate { monate: 1 },
            rechtsgrundlage: "§ 41 Abs. 5 Satz 2 EnWG",
            bezeichnung: "1 Monat".to_owned(),
            nur_zum_monatsersten: false,
        }
    } else {
        Preisanpassungsregime {
            frist: Frist::Tage { tage: 14 },
            rechtsgrundlage: "§ 41 Abs. 5 Satz 2 EnWG",
            bezeichnung: "2 Wochen".to_owned(),
            nur_zum_monatsersten: false,
        }
    }
}

// ── Laufzeit- und Verlängerungsgrenzen (§ 309 Nr. 9 BGB, § 38 EnWG) ───────────

/// A term the statute does not permit, named so the API can return it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Laufzeitverstoss {
    pub regel: &'static str,
    pub rechtsgrundlage: &'static str,
    pub detail: String,
}

/// Check a proposed contract term against the statutory caps.
///
/// § 309 Nr. 9 BGB voids the clause, not the contract, so these are refused at
/// the API boundary: a contract that cannot be enforced as written must not be
/// created as written. Business customers are outside § 309 (§ 310 Abs. 1 BGB),
/// so only the § 38 EnWG Ersatzversorgungs-limit applies to them.
#[must_use]
pub fn pruefe_laufzeit(
    haushaltskunde: bool,
    vertragsart: Vertragsart,
    vertragsbeginn: Date,
    vertragsende: Option<Date>,
    kuendigungsfrist_monate: i32,
    auto_renewal: bool,
    renewal_monate: i32,
) -> Vec<Laufzeitverstoss> {
    let mut out = Vec::new();

    if vertragsart == Vertragsart::Ersatzversorgung {
        let grenze = add_months(vertragsbeginn, 3);
        match vertragsende {
            None => out.push(Laufzeitverstoss {
                regel: "Ersatzversorgung ohne Ende",
                rechtsgrundlage: "§ 38 Abs. 4 EnWG",
                detail: format!(
                    "die Ersatzversorgung endet spätestens am {grenze}; ein offenes Vertragsende ist nicht zulässig"
                ),
            }),
            Some(ende) if ende > grenze => out.push(Laufzeitverstoss {
                regel: "Ersatzversorgung länger als 3 Monate",
                rechtsgrundlage: "§ 38 Abs. 4 EnWG",
                detail: format!("vertragsende {ende} liegt nach der Höchstdauer {grenze}"),
            }),
            Some(_) => {}
        }
    }

    if !haushaltskunde {
        return out;
    }

    // lit. a — an initial term binding the consumer for more than two years.
    if let Some(ende) = vertragsende {
        let grenze = add_months(vertragsbeginn, 24);
        if ende > grenze {
            out.push(Laufzeitverstoss {
                regel: "Erstlaufzeit über 24 Monate",
                rechtsgrundlage: "§ 309 Nr. 9 lit. a BGB",
                detail: format!("vertragsende {ende} liegt nach der Höchstlaufzeit {grenze}"),
            });
        }
    }

    // lit. c — more than one month's notice before the end of the initial term.
    if kuendigungsfrist_monate > 1 {
        out.push(Laufzeitverstoss {
            regel: "Kündigungsfrist über einen Monat",
            rechtsgrundlage: "§ 309 Nr. 9 lit. c BGB",
            detail: format!(
                "kuendigungsfrist_monate = {kuendigungsfrist_monate}; für Verbraucher ist höchstens 1 Monat zulässig"
            ),
        });
    }

    // lit. b — a tacit extension is only lawful into an open-ended contract.
    // `renewal_monate > 0` is exactly the fixed-term extension the clause bans;
    // the lawful form is modelled as `renewal_monate = 0`, see [`verlaengerung`].
    if auto_renewal && renewal_monate > 0 {
        out.push(Laufzeitverstoss {
            regel: "befristete stillschweigende Verlängerung",
            rechtsgrundlage: "§ 309 Nr. 9 lit. b BGB",
            detail: format!(
                "renewal_monate = {renewal_monate}; eine stillschweigende Verlängerung ist nur \
                 auf unbestimmte Zeit mit höchstens einmonatiger Kündigungsfrist zulässig \
                 (renewal_monate = 0)"
            ),
        });
    }

    out
}

/// What a contract looks like after its term runs out with `auto_renewal` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verlaengerung {
    /// § 309 Nr. 9 lit. b BGB: the contract continues **unbefristet** and the
    /// customer may terminate at any time on one month's notice. This is the
    /// only lawful tacit extension of a consumer contract — extending it by
    /// another fixed term is what the clause forbids.
    Unbefristet,
    /// A further fixed term ending on this date. Business contracts only.
    Befristet(Date),
}

/// The new term for a contract whose `vertragsende` has passed.
///
/// Repeats until the contract is in force again so a worker that missed a few
/// days catches up in one pass instead of one term per run.
#[must_use]
pub fn verlaengerung(
    haushaltskunde: bool,
    vertragsende: Date,
    renewal_monate: i32,
    heute: Date,
) -> Verlaengerung {
    if haushaltskunde || renewal_monate <= 0 {
        return Verlaengerung::Unbefristet;
    }
    let mut ende = vertragsende;
    // Twelve terms is more catch-up than any real outage needs and bounds the loop.
    for _ in 0..12 {
        ende = add_months(ende, renewal_monate);
        if ende > heute {
            break;
        }
    }
    Verlaengerung::Befristet(ende)
}

// ── Kundenidentität (`E_0624` Prüfschritt 50) ─────────────────────────────────

/// Whether the customer a market message names is the one on the contract.
///
/// `E_0624` Prüfschritt 50 asks „Ist der Kunde aus der Anfrage zur Beendigung
/// der Zuordnung identisch mit dem Kunden beim LFA?" and the two answers move
/// in opposite directions: `Ja` refuses the Einzug (`A32`), `Nein` walks on
/// toward releasing the Marktlokation (`A34`). Neither may be produced from a
/// guess, so the comparison publishes a third outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kundenidentitaet {
    /// Every name token matches — the same customer.
    Identisch,
    /// No token matches — a different customer.
    Verschieden,
    /// Some tokens match and some do not, or one side has no name.
    ///
    /// The common case is a family member moving in: same Nachname, different
    /// Vorname. It is exactly the case a rule cannot settle, and the one where
    /// both wrong answers are expensive — so it goes to an operator.
    Unklar,
}

impl Kundenidentitaet {
    /// The tri-state as `mako_pruefung::Bekannt` reads it over the wire.
    #[must_use]
    pub fn as_option(self) -> Option<bool> {
        match self {
            Self::Identisch => Some(true),
            Self::Verschieden => Some(false),
            Self::Unklar => None,
        }
    }
}

/// Normalise one name into comparable tokens.
///
/// The wire and the contract database do not agree on shape and cannot be made
/// to: `SG12 NAD+Z09` splits a person across up to five interchangeable `C080`
/// components under Namensformat `Z01` — „Mustermann", „Erika" — while BO4E
/// stores `vorname` and `nachname`, and a company under `Z02` arrives as one
/// string. So the comparison is on the **set** of tokens, not on their order.
///
/// Folding: case, the German umlauts and `ß`, punctuation, and the legal-form
/// suffixes that a Firmenbezeichnung carries on one side and not the other.
fn name_tokens(name: &str) -> std::collections::BTreeSet<String> {
    /// Rechtsformzusätze — noise for an identity comparison, and written
    /// inconsistently („GmbH & Co. KG" / „GmbH und Co KG").
    const RECHTSFORM: &[&str] = &[
        "gmbh", "ag", "kg", "ohg", "gbr", "ug", "se", "ev", "eg", "mbh", "co", "und", "and",
    ];
    name.chars()
        .map(|c| match c {
            'ä' | 'Ä' => 'a',
            'ö' | 'Ö' => 'o',
            'ü' | 'Ü' => 'u',
            c if c.is_alphanumeric() => c.to_ascii_lowercase(),
            _ => ' ',
        })
        .collect::<String>()
        .replace('ß', "ss")
        .split_whitespace()
        .filter(|t| !t.is_empty() && !RECHTSFORM.contains(t))
        .map(ToOwned::to_owned)
        .collect()
}

/// Kölner Phonetik (Postel 1969) — the phonetic code of one normalised token.
///
/// Soundex and Metaphone are tuned to English and miss the German pairs that
/// matter here: `Meyer`/`Maier`/`Mayer` are one name, and Jaro-Winkler scores
/// them ≈ 0.87 — under any threshold that does not also match unrelated names.
/// Kölner Phonetik maps all three to `67`.
///
/// Input is already folded by [`name_tokens`]: lowercase ASCII, umlauts and `ß`
/// resolved.
fn koelner_phonetik(token: &str) -> String {
    let c: Vec<char> = token.chars().collect();
    let mut out = String::new();
    for (i, &ch) in c.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| c.get(p)).copied();
        let next = c.get(i + 1).copied();
        let code = match ch {
            'a' | 'e' | 'i' | 'j' | 'o' | 'u' | 'y' => "0",
            'h' => "",
            'b' => "1",
            'p' => {
                if next == Some('h') {
                    "3"
                } else {
                    "1"
                }
            }
            'd' | 't' => {
                if matches!(next, Some('c' | 's' | 'z')) {
                    "8"
                } else {
                    "2"
                }
            }
            'f' | 'v' | 'w' => "3",
            'g' | 'k' | 'q' => "4",
            'c' => match (i, prev, next) {
                (0, _, Some('a' | 'h' | 'k' | 'l' | 'o' | 'q' | 'r' | 'u' | 'x')) => "4",
                (0, ..) => "8",
                (_, Some('s' | 'z'), _) => "8",
                (_, _, Some('a' | 'h' | 'k' | 'o' | 'q' | 'u' | 'x')) => "4",
                _ => "8",
            },
            'x' => {
                if matches!(prev, Some('c' | 'k' | 'q')) {
                    "8"
                } else {
                    "48"
                }
            }
            'l' => "5",
            'm' | 'n' => "6",
            'r' => "7",
            's' | 'z' => "8",
            _ => "",
        };
        for d in code.chars() {
            if !out.ends_with(d) {
                out.push(d);
            }
        }
    }
    // Every `0` but a leading one is dropped.
    let mut it = out.chars();
    let head: String = it.by_ref().take(1).collect();
    head + &it.filter(|d| *d != '0').collect::<String>()
}

/// Whether two normalised name tokens plausibly denote the same name part.
///
/// Three tests, cheapest first: equality, Jaro-Winkler ≥ 0.90 (Winkler's own
/// threshold for personal names — it catches transpositions and single-letter
/// typos while leaving unrelated names well below), and Kölner Phonetik for the
/// German spelling variants a string metric cannot see.
///
/// The phonetic test needs three characters: on shorter tokens — initials,
/// „de", „van" — it collides indiscriminately.
fn tokens_similar(a: &str, b: &str) -> bool {
    a == b
        || strsim::jaro_winkler(a, b) >= 0.90
        || (a.chars().count() >= 3
            && b.chars().count() >= 3
            && koelner_phonetik(a) == koelner_phonetik(b))
}

/// Compare the customer a message names against the contract holder.
///
/// # Where the fuzziness goes
///
/// Similarity widens [`Kundenidentitaet::Unklar`] and nothing else — it never
/// produces an answer that exact comparison would not:
///
/// - **`Identisch`** needs the token sets to be *equal*. It drives `A32`, an
///   Ablehnung; asserting that two customers are the same person on a
///   similarity score is the guess this crate exists to avoid.
/// - **`Verschieden`** needs *no* token pair to be similar — not merely none to
///   be equal. It drives the walk toward `A34`, which releases the
///   Marktlokation, so „Meier" against „Meyer" must not reach it.
/// - Everything between is an operator's call.
///
/// See [`Kundenidentitaet`] for why a partial match is not an answer.
#[must_use]
pub fn kundenidentitaet(aus_anfrage: Option<&str>, beim_lfa: Option<&str>) -> Kundenidentitaet {
    let (Some(a), Some(b)) = (aus_anfrage, beim_lfa) else {
        return Kundenidentitaet::Unklar;
    };
    let (a, b) = (name_tokens(a), name_tokens(b));
    if a.is_empty() || b.is_empty() {
        return Kundenidentitaet::Unklar;
    }
    if a == b {
        return Kundenidentitaet::Identisch;
    }
    let overlap = a.iter().any(|x| b.iter().any(|y| tokens_similar(x, y)));
    if overlap {
        Kundenidentitaet::Unklar
    } else {
        Kundenidentitaet::Verschieden
    }
}

// ── Kalenderarithmetik ────────────────────────────────────────────────────────

/// Add `months` calendar months, clamping the day into the target month.
///
/// Terms of 1, 3 or 6 months are not expressible in whole years, and the day
/// must survive the short months: 31 January plus one month is 28 February.
#[must_use]
pub fn add_months(from: Date, months: i32) -> Date {
    let total = from.month() as i32 + months;
    let jahre = (total - 1).div_euclid(12);
    let monat = u8::try_from((total - 1).rem_euclid(12) + 1).unwrap_or(1);
    let jahr = from.year() + jahre;
    let tag = from.day().min(tage_im_monat(jahr, monat));
    time::Month::try_from(monat)
        .ok()
        .and_then(|m| Date::from_calendar_date(jahr, m, tag).ok())
        .unwrap_or(from)
}

/// The first day of the month after `from` — where § 5 Abs. 2 GVV requires a
/// price change to land.
#[must_use]
pub fn naechster_monatserster(from: Date) -> Date {
    let erster = Date::from_calendar_date(from.year(), from.month(), 1).unwrap_or(from);
    add_months(erster, 1)
}

fn tage_im_monat(jahr: i32, monat: u8) -> u8 {
    match monat {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if jahr % 4 == 0 && (jahr % 100 != 0 || jahr % 400 == 0) => 29,
        2 => 28,
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::{Kundenidentitaet, kundenidentitaet};

    /// The wire and the contract database never agree on shape: `SG12 NAD+Z09`
    /// under Namensformat `Z01` splits a person across `C080`'s
    /// interchangeable components — „Mustermann", „Erika" — while BO4E stores
    /// `vorname`/`nachname`. An order-sensitive comparison would call the same
    /// person two different customers on every single Einzug.
    #[test]
    fn a_person_matches_whichever_order_the_parts_arrive_in() {
        assert_eq!(
            kundenidentitaet(Some("Mustermann Erika"), Some("Erika Mustermann")),
            Kundenidentitaet::Identisch
        );
        assert_eq!(
            kundenidentitaet(Some("MUSTERMANN  ERIKA"), Some("Erika Mustermann")),
            Kundenidentitaet::Identisch
        );
    }

    /// Umlauts and `ß` fold, because one side is an EDIFACT payload in a
    /// restricted repertoire and the other is a database column.
    #[test]
    fn umlauts_and_eszett_fold() {
        assert_eq!(
            kundenidentitaet(Some("Weiss Jürgen"), Some("Jürgen Weiß")),
            Kundenidentitaet::Identisch
        );
    }

    /// A Rechtsformzusatz is written inconsistently on the two sides and says
    /// nothing about identity.
    #[test]
    fn a_rechtsform_suffix_is_not_part_of_the_identity() {
        assert_eq!(
            kundenidentitaet(
                Some("Muster Energie GmbH & Co. KG"),
                Some("Muster Energie GmbH und Co KG")
            ),
            Kundenidentitaet::Identisch
        );
    }

    /// Nothing in common — not even phonetically — is an answer: a different
    /// customer moved in.
    #[test]
    fn disjoint_names_are_a_different_customer() {
        assert_eq!(
            kundenidentitaet(Some("Schmidt Anna"), Some("Erika Mustermann")),
            Kundenidentitaet::Verschieden
        );
        assert_eq!(
            kundenidentitaet(Some("Nordwind Energie GmbH"), Some("Erika Mustermann")),
            Kundenidentitaet::Verschieden
        );
    }

    /// **The case that must not be guessed.** A family member moving in shares
    /// the Nachname and nothing else, and the two possible answers move in
    /// opposite directions: `A32` refuses the Einzug, `A34` releases the
    /// Marktlokation. Prüfschritt 50 gets an operator instead.
    #[test]
    fn a_shared_surname_alone_is_not_an_answer() {
        assert_eq!(
            kundenidentitaet(Some("Mustermann Max"), Some("Erika Mustermann")),
            Kundenidentitaet::Unklar
        );
    }

    /// Kölner Phonetik maps the German spelling variants a string metric
    /// cannot see: `Meyer`/`Maier`/`Mayer` are one name and score ≈ 0.87 on
    /// Jaro-Winkler, under any threshold that does not also match unrelated
    /// names.
    #[test]
    fn the_phonetic_code_folds_german_spelling_variants() {
        use super::koelner_phonetik;
        let meyer = koelner_phonetik("meyer");
        for variant in ["maier", "mayer", "meier", "mayr"] {
            assert_eq!(koelner_phonetik(variant), meyer, "{variant}");
        }
        assert_eq!(koelner_phonetik("schmidt"), koelner_phonetik("schmitt"));
        assert_ne!(koelner_phonetik("mustermann"), koelner_phonetik("schmidt"));
    }

    /// **Similarity only ever widens `Unklar`.** A spelling variant must not
    /// reach `Verschieden`: that answer walks `E_0624` on toward `A34`, which
    /// releases the Marktlokation.
    #[test]
    fn a_spelling_variant_is_not_a_different_customer() {
        for (wire, contract) in [
            ("Meier", "Meyer"),
            ("Schmidt", "Schmitt"),
            // A single transposed letter — Jaro-Winkler's own case.
            ("Mustermann", "Mustermann"),
            ("Muhlbauer", "Mühlbauer"),
        ] {
            assert_ne!(
                kundenidentitaet(Some(wire), Some(contract)),
                Kundenidentitaet::Verschieden,
                "{wire} / {contract}"
            );
        }
    }

    /// …and it must not reach `Identisch` either. `A32` is an Ablehnung, and a
    /// similarity score is not a statement that two customers are one person.
    #[test]
    fn a_spelling_variant_is_not_an_assertion_of_identity() {
        assert_eq!(
            kundenidentitaet(Some("Meier Anna"), Some("Anna Meyer")),
            Kundenidentitaet::Unklar
        );
    }

    /// A missing name on either side is not evidence of anything.
    #[test]
    fn a_missing_name_is_unknown() {
        assert_eq!(
            kundenidentitaet(None, Some("Erika Mustermann")),
            Kundenidentitaet::Unklar
        );
        assert_eq!(
            kundenidentitaet(Some("Erika Mustermann"), None),
            Kundenidentitaet::Unklar
        );
        assert_eq!(
            kundenidentitaet(Some("   "), Some("Erika Mustermann")),
            Kundenidentitaet::Unklar
        );
    }

    use super::*;
    use time::macros::date;

    // ── Kündigungsfristen ────────────────────────────────────────────────────

    #[test]
    fn grundversorgung_kuendigt_auf_zwei_wochen() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Grundversorgung,
            true,
            Kuendigungsgrund::Ordentlich,
            3,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 03 - 24));
        assert_eq!(f.rechtsgrundlage, "§ 20 Abs. 1 StromGVV / GasGVV");
    }

    #[test]
    fn die_ersatzversorgung_kuendigt_wie_die_grundversorgung() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Ersatzversorgung,
            true,
            Kuendigungsgrund::Ordentlich,
            12,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 03 - 24));
    }

    #[test]
    fn der_umzug_gibt_dem_haushaltskunden_sechs_wochen() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Sondervertrag,
            true,
            Kuendigungsgrund::Umzug,
            12,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 04 - 21));
        assert_eq!(f.rechtsgrundlage, "§ 41b Abs. 5 EnWG");
    }

    #[test]
    fn der_umzug_eines_gewerbekunden_bleibt_auf_der_vertraglichen_frist() {
        // § 41b Abs. 5 EnWG grants the right to Haushaltskunden only.
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Sondervertrag,
            false,
            Kuendigungsgrund::Umzug,
            3,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 06 - 10));
    }

    #[test]
    fn die_sonderkuendigung_endet_am_tag_der_preisaenderung() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Sondervertrag,
            true,
            Kuendigungsgrund::Preisanpassung,
            12,
            Some(date!(2026 - 04 - 01)),
        );
        assert_eq!(f.fruehestens, date!(2026 - 04 - 01));
        assert_eq!(f.rechtsgrundlage, "§ 41 Abs. 5 Satz 4 EnWG");
    }

    #[test]
    fn eine_dreimonatige_frist_wird_gegen_verbraucher_auf_einen_monat_gekuerzt() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 31),
            Vertragsart::Sondervertrag,
            true,
            Kuendigungsgrund::Ordentlich,
            3,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 04 - 30)); // Tag geklammert
        assert!(f.rechtsgrundlage.contains("§ 309 Nr. 9 lit. c BGB"));
    }

    #[test]
    fn ein_gewerbekunde_behaelt_die_dreimonatige_frist() {
        let f = kuendigungsfrist(
            date!(2026 - 03 - 10),
            Vertragsart::Sondervertrag,
            false,
            Kuendigungsgrund::Ordentlich,
            3,
            None,
        );
        assert_eq!(f.fruehestens, date!(2026 - 06 - 10));
        assert_eq!(f.rechtsgrundlage, "vertragliche Kündigungsfrist");
    }

    // ── Preisanpassung ───────────────────────────────────────────────────────

    #[test]
    fn die_drei_vorlauffristen_stimmen_mit_der_norm_ueberein() {
        let gv = preisanpassungsregime(Vertragsart::Grundversorgung, true);
        assert_eq!(gv.frist, Frist::Tage { tage: 42 });
        assert!(gv.nur_zum_monatsersten);

        let haushalt = preisanpassungsregime(Vertragsart::Sondervertrag, true);
        assert_eq!(haushalt.frist, Frist::Monate { monate: 1 });
        assert_eq!(haushalt.rechtsgrundlage, "§ 41 Abs. 5 Satz 2 EnWG");
        assert!(!haushalt.nur_zum_monatsersten);

        let gewerbe = preisanpassungsregime(Vertragsart::Sondervertrag, false);
        assert_eq!(gewerbe.frist, Frist::Tage { tage: 14 });
    }

    /// „1 Monat" is a calendar month under § 188 Abs. 2 BGB, not 30 days.
    ///
    /// A flat 30 days is one day **short** of the statute for every 31-day
    /// month — a notice given on 1 January would be admitted for 31 January
    /// when the statute requires 1 February.
    #[test]
    fn ein_monat_ist_ein_kalendermonat_kein_30_tage_fenster() {
        let monat = Frist::Monate { monate: 1 };

        // 31-day month: 30 days would have been a day short.
        assert_eq!(
            monat.fruehestens_ab(date!(2026 - 01 - 01)),
            date!(2026 - 02 - 01)
        );
        assert!(!monat.gewahrt(date!(2026 - 01 - 01), date!(2026 - 01 - 31)));
        assert!(monat.gewahrt(date!(2026 - 01 - 01), date!(2026 - 02 - 01)));

        // 28-day month: 30 days would have over-required by two.
        assert_eq!(
            monat.fruehestens_ab(date!(2026 - 02 - 01)),
            date!(2026 - 03 - 01)
        );
        assert!(monat.gewahrt(date!(2026 - 02 - 01), date!(2026 - 03 - 01)));
    }

    /// § 188 Abs. 3 BGB — a month from the 31st ends on the target month's last
    /// day when it has no 31st.
    #[test]
    fn ein_monat_vom_monatsletzten_klemmt_auf_den_letzten_tag() {
        let monat = Frist::Monate { monate: 1 };
        assert_eq!(
            monat.fruehestens_ab(date!(2026 - 01 - 31)),
            date!(2026 - 02 - 28)
        );
        assert_eq!(
            monat.fruehestens_ab(date!(2026 - 03 - 31)),
            date!(2026 - 04 - 30)
        );
        // Leap year.
        assert_eq!(
            monat.fruehestens_ab(date!(2028 - 01 - 31)),
            date!(2028 - 02 - 29)
        );
    }

    /// Weeks are exact multiples of seven days, so they stay day-denominated.
    #[test]
    fn wochenfristen_bleiben_tagesfristen() {
        assert_eq!(
            Frist::Tage { tage: 42 }.fruehestens_ab(date!(2026 - 01 - 01)),
            date!(2026 - 02 - 12)
        );
    }

    /// § 5 Abs. 2 GVV: a Grundversorgungs-price change takes effect only at a
    /// month start, so the six-week date rolls forward to the next one.
    #[test]
    fn die_grundversorgung_wirkt_nur_zum_monatsersten() {
        let gv = preisanpassungsregime(Vertragsart::Grundversorgung, true);
        // 1 Jan + 42 days = 12 Feb, which is not a Monatserster.
        assert_eq!(
            gv.fruehestens_wirksam(date!(2026 - 01 - 01)),
            date!(2026 - 03 - 01)
        );
        // A six-week date that already lands on the 1st stays put.
        assert_eq!(
            gv.fruehestens_wirksam(date!(2025 - 12 - 21)),
            date!(2026 - 02 - 01)
        );
    }

    // ── § 309 Nr. 9 BGB ──────────────────────────────────────────────────────

    #[test]
    fn eine_dreissigmonatige_erstlaufzeit_ist_fuer_verbraucher_unzulaessig() {
        let v = pruefe_laufzeit(
            true,
            Vertragsart::Sondervertrag,
            date!(2026 - 01 - 01),
            Some(date!(2028 - 07 - 01)),
            1,
            false,
            0,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rechtsgrundlage, "§ 309 Nr. 9 lit. a BGB");
    }

    #[test]
    fn die_gleiche_laufzeit_ist_im_b2b_zulaessig() {
        let v = pruefe_laufzeit(
            false,
            Vertragsart::Sondervertrag,
            date!(2026 - 01 - 01),
            Some(date!(2028 - 07 - 01)),
            3,
            true,
            12,
        );
        assert!(v.is_empty());
    }

    #[test]
    fn eine_befristete_stillschweigende_verlaengerung_ist_fuer_verbraucher_unzulaessig() {
        let v = pruefe_laufzeit(
            true,
            Vertragsart::Sondervertrag,
            date!(2026 - 01 - 01),
            Some(date!(2027 - 01 - 01)),
            1,
            true,
            12,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rechtsgrundlage, "§ 309 Nr. 9 lit. b BGB");
    }

    #[test]
    fn die_unbefristete_verlaengerung_ist_zulaessig() {
        let v = pruefe_laufzeit(
            true,
            Vertragsart::Sondervertrag,
            date!(2026 - 01 - 01),
            Some(date!(2027 - 01 - 01)),
            1,
            true,
            0,
        );
        assert!(v.is_empty());
    }

    #[test]
    fn die_ersatzversorgung_endet_spaetestens_nach_drei_monaten() {
        let v = pruefe_laufzeit(
            true,
            Vertragsart::Ersatzversorgung,
            date!(2026 - 01 - 01),
            Some(date!(2026 - 06 - 01)),
            1,
            false,
            0,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rechtsgrundlage, "§ 38 Abs. 4 EnWG");
    }

    #[test]
    fn eine_ersatzversorgung_ohne_ende_wird_abgelehnt() {
        let v = pruefe_laufzeit(
            true,
            Vertragsart::Ersatzversorgung,
            date!(2026 - 01 - 01),
            None,
            1,
            false,
            0,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rechtsgrundlage, "§ 38 Abs. 4 EnWG");
    }

    // ── Verlängerung ─────────────────────────────────────────────────────────

    #[test]
    fn ein_verbrauchervertrag_verlaengert_sich_auf_unbestimmte_zeit() {
        assert_eq!(
            verlaengerung(true, date!(2026 - 01 - 31), 12, date!(2026 - 02 - 01)),
            Verlaengerung::Unbefristet
        );
    }

    #[test]
    fn ein_b2b_vertrag_verlaengert_sich_um_die_vereinbarte_laufzeit() {
        assert_eq!(
            verlaengerung(false, date!(2026 - 01 - 31), 12, date!(2026 - 02 - 01)),
            Verlaengerung::Befristet(date!(2027 - 01 - 31))
        );
    }

    #[test]
    fn mehrere_verpasste_laufzeiten_werden_in_einem_durchlauf_nachgeholt() {
        assert_eq!(
            verlaengerung(false, date!(2023 - 01 - 31), 12, date!(2026 - 02 - 01)),
            Verlaengerung::Befristet(date!(2027 - 01 - 31))
        );
    }

    // ── Kalenderarithmetik ───────────────────────────────────────────────────

    #[test]
    fn der_tag_wird_in_den_kuerzeren_monat_geklammert() {
        assert_eq!(add_months(date!(2026 - 01 - 31), 1), date!(2026 - 02 - 28));
        assert_eq!(add_months(date!(2028 - 01 - 31), 1), date!(2028 - 02 - 29));
    }

    #[test]
    fn der_jahreswechsel_traegt_ueber() {
        assert_eq!(add_months(date!(2026 - 11 - 15), 3), date!(2027 - 02 - 15));
        assert_eq!(add_months(date!(2026 - 03 - 15), -4), date!(2025 - 11 - 15));
    }

    #[test]
    fn der_naechste_monatserste_folgt_dem_dezember_ins_neue_jahr() {
        assert_eq!(
            naechster_monatserster(date!(2026 - 12 - 17)),
            date!(2027 - 01 - 01)
        );
    }
}
