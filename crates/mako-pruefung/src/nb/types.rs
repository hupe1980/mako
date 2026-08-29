//! Inputs and outputs of the Netzbetreiber's `E_0622` / `E_0607` decisions.
//!
//! All types are `Clone + Debug + Serialize + Deserialize` so that callers can
//! log inputs/outputs and store audit records without extra conversions.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use mako_markt::domain::Sparte;

pub use crate::antwort::{AntwortDetail, RejectReason};
use crate::codes::{AntwortCode, Cluster};

// ── Marktlokationsart ─────────────────────────────────────────────────────────

/// Which kind of Marktlokation an Anwendungsfall addresses.
///
/// `E_0622` Prüfschritt 10 branches the entire tree on this question, and the
/// two branches share **no** Antwortcode: „andere Anmeldung in Bearbeitung" is
/// `A06` for a verbrauchende Marktlokation and `A45` for an erzeugende one.
/// Deciding it from a boolean („is this an EEG MaLo?") collapses the ruhende
/// case into the wrong branch, which is why it is an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Marktlokationsart {
    /// Verbrauchende Marktlokation — the ordinary consumption case.
    Verbrauchend,
    /// Ruhende Marktlokation — a Marktlokation being integrated into, or
    /// released from, a Kundenanlage (§ 20 Abs. 1d EnWG bzw. § 10c EEG),
    /// signalled by the `ZAP` Transaktionsgrundergänzung.
    ///
    /// Walks the **same** `E_0622` branch as [`Self::Verbrauchend`]
    /// (Prüfschritt 10 asks for „verbrauchende **oder ruhende**"). A ruhende
    /// Marktlokation is a lawful Anmeldung subject: Prüfschritte 16–28 exist to
    /// check it, and Prüfschritt 30's „nimmt nicht an der Marktkommunikation
    /// teil" names only stillgelegte Marktlokationen and the Modell-2-Zuordnung.
    Ruhend,
    /// Erzeugende Marktlokation or Tranche einer erzeugenden Marktlokation.
    Erzeugend,
}

impl Marktlokationsart {
    /// `true` for the branch `E_0622` reaches through Prüfschritt 10 „ja".
    #[must_use]
    pub const fn ist_verbrauchend_oder_ruhend(self) -> bool {
        matches!(self, Self::Verbrauchend | Self::Ruhend)
    }
}

// ── Veräußerungsform ──────────────────────────────────────────────────────────

/// UTILMD `SG10 CCI+Z22++<code>` DE 7037 — the Veräußerungsform of an
/// erzeugende Marktlokation (UTILMD MIG Strom S2.2, Klassentyp `Z22`
/// „Gesetzliche Kategorie").
///
/// The Vorlauffrist of an Anmeldung erzeugender Marktlokation is decided by the
/// **pair** (bestehende, angemeldete) — GPKE Teil 2 § 2.1.1 „Fristen für die
/// Anmeldung bei EEG-Marktlokationen". A switch into or out of the Marktprämie
/// is a Veräußerungsformwechsel and takes the Monatserster plus a month of
/// lead; staying in the same form does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Veraeusserungsform {
    /// `Z90` — Einspeisevergütung (§ 21 Abs. 1 Nr. 1 EEG 2023) **or**
    /// Ausfallvergütung (§ 21 Abs. 1 Nr. 2 EEG 2023).
    ///
    /// One wire code, two regimes with different Fristen: the Ausfallvergütung
    /// takes the verkürzte 5-Werktage-Vorlauffrist, the uneingeschränkte
    /// Einspeisevergütung the full month. The message cannot tell them apart —
    /// the NB's own EEG-Anlagenregister must, via
    /// [`ErzeugungsAnmeldung::ausfallverguetung`].
    Einspeiseverguetung,
    /// `Z91` — geförderte Direktvermarktung (Marktprämie, § 21 Abs. 1 Nr. 1
    /// EEG 2023).
    Marktpraemie,
    /// `Z92` — sonstige Direktvermarktung, ohne gesetzliche Vergütung.
    SonstigeDirektvermarktung,
    /// `Z94` — KWKG-Vergütung.
    KwkgVerguetung,
}

impl Veraeusserungsform {
    /// The UTILMD `CCI+Z22` DE 7037 code.
    #[must_use]
    pub const fn wire_code(self) -> &'static str {
        match self {
            Self::Einspeiseverguetung => "Z90",
            Self::Marktpraemie => "Z91",
            Self::SonstigeDirektvermarktung => "Z92",
            Self::KwkgVerguetung => "Z94",
        }
    }

    /// Parse a `CCI+Z22` DE 7037 code. Unknown codes yield `None` rather than a
    /// guess — the Vorlauffrist branch turns on this value.
    #[must_use]
    pub fn from_wire_code(code: &str) -> Option<Self> {
        match code {
            "Z90" => Some(Self::Einspeiseverguetung),
            "Z91" => Some(Self::Marktpraemie),
            "Z92" => Some(Self::SonstigeDirektvermarktung),
            "Z94" => Some(Self::KwkgVerguetung),
            _ => None,
        }
    }
}

/// GPKE Teil 2 § 2.1.1 Geschäftsvorfall of a Lieferbeginn an einer erzeugenden
/// Marktlokation. `E_0622` Prüfschritte 300 / 310 branch on it, and each
/// branch has its own Antwortcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Geschaeftsvorfall {
    /// 1 — Zuordnung zur nicht-tranchierten Marktlokation (Tranchengröße 100 %).
    Eins,
    /// 2 — Zuordnung zu einer bestehenden Tranche.
    Zwei,
    /// 3 — Zuordnung unter Bildung einer neuen Tranche (Tranchengröße < 100 %).
    Drei,
}

/// The facts an Anmeldung erzeugender Marktlokation needs beyond the common
/// ones — partly from the message, partly from the NB's own EEG-/KWKG-Register.
///
/// Every field is what `E_0622`'s Prüfschritte 300–830 ask for. When one is
/// absent the engine **escalates** — the branch chooses between six published
/// Vorlauffristen, and none of them is a safe default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErzeugungsAnmeldung {
    /// Geschäftsvorfall 1 / 2 / 3 (`E_0622` Prüfschritte 300 / 310).
    pub geschaeftsvorfall: Geschaeftsvorfall,
    /// The Veräußerungsform the Anmeldung declares — UTILMD `SG10 CCI+Z22`.
    pub angemeldete_veraeusserungsform: Veraeusserungsform,
    /// The Veräußerungsform in force at the Zuordnungsbeginn, from the NB's own
    /// register. `None` when the NB has no record — the Veräußerungsformwechsel
    /// question (`E_0622` Prüfschritt 400 / 600) cannot then be answered.
    pub bestehende_veraeusserungsform: Option<Veraeusserungsform>,
    /// `true` for a „Nicht-EEG-/-KWKG"-Marktlokation (`E_0622` Prüfschritte
    /// 405 / 605 / 805), which takes the ordinary Werktag-Vorlauffrist rather
    /// than the EEG Monatserster rule.
    pub nicht_eeg_kwkg: bool,
    /// `true` when the plant is on the **Ausfallvergütung** (§ 21 Abs. 1 Nr. 2
    /// EEG 2023 / § 38 EEG 2014) rather than the uneingeschränkte
    /// Einspeisevergütung — the „verkürzter Wechsel" of `E_0622` Prüfschritt
    /// 420, whose Vorlauffrist is 5 Werktage instead of a month.
    ///
    /// Both ride wire code `Z90`, so this comes from the NB's register.
    pub ausfallverguetung: bool,
}

impl ErzeugungsAnmeldung {
    /// `E_0622` Prüfschritt 400 / 600 — „Verändert sich die Veräußerungsform
    /// zum Tag des gewünschten Zuordnungsbeginns?"
    ///
    /// `None` when the bestehende Veräußerungsform is unknown.
    #[must_use]
    pub fn ist_veraeusserungsformwechsel(&self) -> Option<bool> {
        self.bestehende_veraeusserungsform
            .map(|b| b != self.angemeldete_veraeusserungsform)
    }
}

// ── AnmeldungAnfrage ──────────────────────────────────────────────────────────

/// Classification of metering point.
///
/// Used to apply the correct Mindestvorlauffrist rule:
/// - `Slp`: SLP (Standardlastprofil) — LFW24 day rule applies (spätester ÜT
///   ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn).
/// - `Rlm`: RLM (Registrierende Lastgangmessung) — 2 Werktage minimum lead.
/// - `Imsys`: intelligentes Messsystem (iMSys) — treated as SLP for Vorlauffrist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Messtyp {
    /// Standardlastprofil metering.
    Slp,
    /// Registrierende Lastgangmessung (interval metering).
    Rlm,
    /// Intelligentes Messsystem.
    Imsys,
}

impl std::fmt::Display for Messtyp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slp => write!(f, "SLP"),
            Self::Rlm => write!(f, "RLM"),
            Self::Imsys => write!(f, "IMSYS"),
        }
    }
}

/// Parsed fields from a `de.mako.process.initiated` event for a Lieferbeginn PID.
///
/// All fields that `mako-pruefung` needs are extracted at the transport boundary
/// by `processd` before calling `evaluate`.  No raw CloudEvent JSON arrives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnmeldungAnfrage {
    /// BDEW Prüfidentifikator:
    /// - `55001` Anmeldung verbrauchende Marktlokation (Strom)
    /// - `55077` Anmeldung erzeugende Marktlokation (Strom)
    /// - `44001` Anmeldung NN (Gas)
    pub pid: u32,
    /// mako process UUID (from `subject` CE field).
    pub process_id: Uuid,
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// GLN of the requesting new Lieferant.
    pub new_supplier_gln: String,
    /// GLN of the grid operator to whom the request is directed.
    ///
    /// Must equal the operator's own GLN; otherwise the event is misdirected.
    pub grid_operator_gln: String,
    /// Bilanzierungsgebiet-EIC provided in the UTILMD message (`LOC+237`).
    ///
    /// `None` when not present in the EDIFACT message (optional in some process variants).
    pub bilanzierungsgebiet: Option<String>,
    /// Requested Lieferbeginn date.
    pub process_date: Date,
    /// Energy commodity (Strom / Gas). Derived from PID.
    pub sparte: Sparte,
    /// Metering classification (SLP / RLM / iMSys).
    ///
    /// For Gas processes this is always `Slp` (GeLi Gas operates on gas MaLos
    /// which are billed as SLP equivalents unless explicitly flagged as RLM Gas).
    pub messtyp: Messtyp,
    /// SG4 STS Transaktionsgrund (DE9013) from the UTILMD, when transmitted —
    /// e.g. `E01` Ein-/Auszug (Umzug), `E03` Lieferantenwechsel, `E06`
    /// Ersatzbelieferung.
    ///
    /// Drives the date-plausibility rules (check 3): GPKE permits a
    /// retroactive Lieferbeginn for Ein-/Auszug within the statutory
    /// backdating window, but not for a regular Wechsel. `None` (legacy
    /// messages or extraction failure) is treated conservatively.
    pub transaktionsgrund: Option<String>,
    /// Which `E_0622` branch the Anwendungsfall belongs to (Prüfschritt 10).
    ///
    /// Derived by the caller from the PID and the UTILMD `SG4 STS+7` DE 9013
    /// Transaktionsgrundergänzung: `ZW4` verbrauchende, `ZW3` erzeugende, `ZAP`
    /// ruhende Marktlokation. PID 55077 **is** the Anwendungsfall „Anmeldung
    /// erzeugende Marktlokation", so it decides the branch on its own.
    pub marktlokationsart: Marktlokationsart,
    /// The extra facts an erzeugende Marktlokation's Vorlauffrist turns on.
    ///
    /// `None` on a verbrauchende or ruhende Marktlokation. `None` on an
    /// erzeugende one means the caller could not resolve them — the engine then
    /// escalates rather than applying one of the six published Fristen at
    /// random.
    #[serde(default)]
    pub erzeugung: Option<ErzeugungsAnmeldung>,
    /// The state of the **Abmeldeanfrage** leg — what `E_0623` Prüfschritte
    /// 20–50 / 410–440 read.
    ///
    /// GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 Prüfschritt 4 decides whether
    /// the NB may confirm at all or must first ask the incumbent LFA to release
    /// the Marktlokation (55010, Nr. 3). Defaults to
    /// [`Abmeldeanfrage::NichtErforderlich`], which is the correct value for an
    /// unassigned Marktlokation and the *only* one a caller that cannot see the
    /// Versorgungsstatus may use.
    #[serde(default)]
    pub abmeldeanfrage: Abmeldeanfrage,
}

// ── Abmeldeanfrage ────────────────────────────────────────────────────────────

/// Where the NB stands on the Anfrage zur Beendigung der Zuordnung.
///
/// The three states are the three answers `E_0623` Prüfschritt 20 / 410 can get,
/// plus the one that is not an answer at all: an Anfrage that is owed and has
/// not gone out yet. Collapsing that into „no Anfrage" is what lets an NB
/// confirm a Lieferantenwechsel without ever consulting the incumbent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Abmeldeanfrage {
    /// No LFA holds the Marktlokation at the Zuordnungsbeginn — Prüfschritt 4
    /// sends the NB straight to Prozessschritt 5, and Prüfschritt 20 answers
    /// „nein".
    #[default]
    NichtErforderlich,
    /// An LFA holds it and the 55010 has not been sent. Not an error: the NB
    /// owes the Anfrage („parallel zu Nr. 2") before it may answer the LFN.
    Erforderlich {
        /// Every LFA to ask. More than one at Geschäftsvorfall 3, where the
        /// Marktlokation is split across Tranchen and the Anfrage goes to all
        /// of them (SD Lieferbeginn Nr. 3).
        lfa_mp_ids: Vec<String>,
    },
    /// The Anfrage went out. `antwort` is `None` while the 09:00 window is still
    /// open **and** after it lapsed — „Verstreicht die Frist, ohne dass eine
    /// Antwort beim NB eingeht, gilt dies als Bestätigung nach Fall a)", so the
    /// two are the same input to the tree and only the clock tells them apart.
    Gestellt {
        /// The LFA's answer, or `None` for silence.
        antwort: Option<LfaAntwort>,
    },
}

/// What the LFA answered the Anfrage zur Beendigung der Zuordnung with
/// (55011 / 55012, `E_0624`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cluster", rename_all = "snake_case")]
pub enum LfaAntwort {
    /// 55011 — the LFA releases the Marktlokation.
    Zustimmung {
        /// The `E_0624` Zustimmungscode (`A31`, `A34`, `A36`, `A38`, `A40`, `A42`).
        code: String,
        /// **Fall b** — the LFA confirms to a Zuordnungsende *earlier* than the
        /// LFN's Zuordnungsbeginn (`A34` „teilt sein Lieferendedatum in der
        /// Antwort mit"). Verbrauchende Marktlokationen only, and it must lie
        /// „mindestens 1 WT nach dem ÜT der Anmeldung"; otherwise the
        /// Zuordnungsende stays the Zuordnungsbeginn der Anmeldung
        /// (GPKE Teil 2 § 2.1.2 Nr. 10).
        #[serde(default, with = "crate::nb::types::date_opt")]
        zuordnungsende: Option<Date>,
    },
    /// 55012 — the LFA refuses.
    Widerspruch {
        /// The `E_0624` Ablehnungscode. `A30` / `A41` („bereits abgemeldet")
        /// is the one that still lets the Anmeldung through.
        code: String,
        /// „Hierbei übermittelt der LFA eine Begründung für den Widerspruch."
        grund: Option<String>,
    },
}

/// `Option<Date>` as an ISO-8601 string, for the wire.
pub(crate) mod date_opt {
    use serde::{Deserialize as _, Deserializer, Serializer};
    use time::Date;
    use time::format_description::well_known::Iso8601;

    #[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S: Serializer>(v: &Option<Date>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_some(
                &d.format(&Iso8601::DATE)
                    .map_err(serde::ser::Error::custom)?,
            ),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Date>, D::Error> {
        let raw = Option::<String>::deserialize(d)?;
        raw.map(|s| Date::parse(&s, &Iso8601::DATE).map_err(serde::de::Error::custom))
            .transpose()
    }
}

// ── TranchenZuordnung ─────────────────────────────────────────────────────────

/// The Tranchen arithmetic `E_0623` Prüfschritte 500–540 run on a
/// Geschäftsvorfall 3.
///
/// A tranchierte Marktlokation is held by several LFA at once, so the question
/// is not „did *the* LFA agree" but „did enough percentage come free". Four of
/// the six `E_0623` outcomes live only here — two Ablehnungen (`A53`, `A54`) and
/// two Zustimmungen that differ in what the NB does next (`A55` triggers
/// „Herstellung einer 100 % LF-Zuordnung", `A56` does not).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TranchenZuordnung {
    /// Prüfschritt 500 — „Wurden Anfragen zur Beendigung der Zuordnung an die
    /// zugeordneten Lieferanten der Tranchen … gestellt?" `false` when the
    /// Marktlokation carried no Tranche to release.
    pub anfragen_gestellt: bool,
    /// Prüfschritt 510.
    pub mindestens_eine_zustimmung: bool,
    /// Prüfschritt 520.
    pub ausreichender_prozentsatz: bool,
    /// Prüfschritt 530 — „Verbleibt ein Anteil im Bilanzkreis des
    /// Netzbetreibers?"
    pub restanteil_im_nb_bilanzkreis: bool,
    /// Prüfschritt 540.
    pub direktvermarktungspflichtig: bool,
    /// The share the LFN registered, for the rejection text.
    pub gewuenschter_prozentsatz: String,
    /// The share that actually came free, for the rejection text.
    pub freigewordener_prozentsatz: String,
}

// ── AbmeldungAnfrage ──────────────────────────────────────────────────────────

/// Parsed fields from a `de.mako.process.initiated` event for an **Abmeldung**
/// PID — Strom `55004`, Gas `44004`.
///
/// Separate from [`AnmeldungAnfrage`] because the two carry different facts: an
/// Anmeldung names the *incoming* supplier and a Bilanzierungsgebiet to check
/// against the grid record; an Abmeldung names the *outgoing* one and nothing
/// to reconcile topology with. Folding them into one struct would leave half
/// its fields meaningless in either direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbmeldungAnfrage {
    /// BDEW Prüfidentifikator: `55004` (Strom) or `44004` (Gas).
    pub pid: u32,
    /// mako process UUID (from the CloudEvent `subject`).
    pub process_id: Uuid,
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// MP-ID of the supplier ending the assignment.
    pub lf_mp_id: String,
    /// GLN of the grid operator the Abmeldung is directed to.
    pub grid_operator_gln: String,
    /// Requested Zuordnungsende („Abmeldedatum").
    pub abmeldedatum: Date,
    /// Energy commodity, derived from the PID.
    pub sparte: Sparte,
    /// Metering classification — drives the Gas retroactivity rules.
    pub messtyp: Messtyp,
    /// SG4 STS Transaktionsgrund (DE9013) — `E01`/`E02` Auszug, `E03`
    /// Lieferantenwechsel. Drives the Gas date rules and the `A09`/`A10` split.
    pub transaktionsgrund: Option<String>,
    /// Which `E_0607` branch the Abmeldung belongs to (Prüfschritt 10 asks for
    /// „verbrauchende **oder ruhende** Marktlokation").
    pub marktlokationsart: Marktlokationsart,
    /// The Veräußerungsform facts, when the Abmeldung names an erzeugende
    /// Marktlokation. `None` makes the Vorlauffrist branch escalate.
    #[serde(default)]
    pub erzeugung: Option<ErzeugungsAnmeldung>,
}

// ── MaloGridRecord ────────────────────────────────────────────────────────────

/// NB grid topology record for a MaLo.
///
/// Written by the NB's NIS/GIS adapter or provisioned manually via
/// `PUT /api/v1/malos/{id}/grid` on `marktd`. Read by `processd` NB module.
///
/// NOTE: This is NOT MaStR data. MaStR covers generation/consumption units,
/// not NB grid topology or Bilanzierungsgebiet assignments.
///
/// Absence of this record triggers `NbEntscheidung::Escalate` (rule 1) — the
/// NB cannot auto-decide without grid topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloGridRecord {
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// GLN of the Netzbetreiber that owns this MaLo.
    pub nb_mp_id: String,
    /// Bilanzierungsgebiet-EIC (`LOC+237` in UTILMD).
    ///
    /// `None` means the Bilanzierungsgebiet is unknown — check 4 is skipped
    /// (treated as passing) when both this field and the UTILMD value are `None`.
    pub bilanzierungsgebiet: Option<String>,
    /// Netzgebiet code (optional; NB-specific identifier).
    pub netzgebiet: Option<String>,
}

// ── NbEntscheidung ───────────────────────────────────────────────────────────

/// Outcome of an NB decision tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NbEntscheidung {
    /// Every applicable Prüfschritt passed. Carries the **Zustimmungscode** the
    /// Bestätigung must state.
    ///
    /// „Accept" is not the absence of a code: the AHB marks `SG4 STS+E01` Muss
    /// on every Antwortnachricht, so a Bestätigung without one is a malformed
    /// UTILMD. The code is `A51` / `A58` / `A55` / `A56` (`E_0623`) for Strom
    /// and `E15` (`G_0012`) for Gas.
    Accept(AntwortDetail),
    /// A deterministic, verifiable Prüfschritt failed.
    ///
    /// Dispatch `ablehnen` with `reason.antwortcode` — it renders into
    /// `SG4 STS+E01++<code>:<ebd>` of the answering UTILMD.
    Reject(RejectReason),
    /// Validation could not complete — data is missing or ambiguous.
    ///
    /// Do NOT auto-decide. Write `anmeldung_decisions` with
    /// `decision = "Escalate"` and alert the operator.
    Escalate {
        /// Human-readable explanation for the operator alert.
        reason: String,
    },
    /// `E_0622` passed, but the Marktlokation is assigned at the
    /// Zuordnungsbeginn — GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1
    /// **Prüfschritt 4** sends the NB to Prozessschritt 3 first.
    ///
    /// **Not an error and not an escalation.** Send the Anfrage zur Beendigung
    /// der Zuordnung (55010 / 44010) to every named LFA, then decide again once
    /// the answer arrives or the 09:00 window lapses — silence counts as
    /// Zustimmung, so a lapsed window is a *result*, not a timeout.
    AnfrageErforderlich {
        /// Every LFA to ask. More than one at Geschäftsvorfall 3.
        lfa_mp_ids: Vec<String>,
        /// The Zuordnungsende to request — the Zuordnungsbeginn of this
        /// Anmeldung (SD Lieferbeginn Nr. 3).
        zuordnungsende: Date,
    },
}

impl NbEntscheidung {
    /// Build an `Accept` from a published Zustimmungscode.
    ///
    /// # Panics
    ///
    /// In debug builds, when `code` is an Ablehnung.
    #[must_use]
    pub fn accept(tree: &'static str, code: &'static AntwortCode) -> Self {
        debug_assert_eq!(
            code.cluster,
            Cluster::Zustimmung,
            "{} is an Ablehnungscode and cannot carry a Bestätigung",
            code.code
        );
        Self::Accept(AntwortDetail::new(tree, code))
    }

    /// The Antwortcode this decision puts on the wire, for either cluster.
    #[must_use]
    pub fn antwortcode(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => Some(&a.antwortcode),
            Self::Reject(r) => Some(&r.antwort.antwortcode),
            Self::Escalate { .. } | Self::AnfrageErforderlich { .. } => None,
        }
    }

    /// The EBD the Antwortcode belongs to.
    #[must_use]
    pub fn ebd(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => a.ebd.as_deref(),
            Self::Reject(r) => r.antwort.ebd.as_deref(),
            Self::Escalate { .. } | Self::AnfrageErforderlich { .. } => None,
        }
    }

    /// Returns `true` if the decision is `Accept`.
    #[must_use]
    pub const fn is_accept(&self) -> bool {
        matches!(self, Self::Accept(_))
    }

    /// Returns `true` if the decision is `Reject`.
    #[must_use]
    pub const fn is_reject(&self) -> bool {
        matches!(self, Self::Reject(_))
    }

    /// Returns `true` if the decision requires operator escalation.
    #[must_use]
    pub const fn is_escalate(&self) -> bool {
        matches!(self, Self::Escalate { .. })
    }

    /// Returns `true` when the NB must first ask the LFA to release the
    /// Marktlokation.
    #[must_use]
    pub const fn needs_abmeldeanfrage(&self) -> bool {
        matches!(self, Self::AnfrageErforderlich { .. })
    }
}

// ── Conversion from mako-markt repository type ────────────────────────────────

impl From<mako_markt::repository::MaloGridRecord> for MaloGridRecord {
    fn from(r: mako_markt::repository::MaloGridRecord) -> Self {
        Self {
            malo_id: r.malo_id.to_string(),
            nb_mp_id: r.nb_mp_id,
            bilanzierungsgebiet: r.bilanzierungsgebiet,
            netzgebiet: r.netzgebiet,
        }
    }
}

impl From<&mako_markt::repository::MaloGridRecord> for MaloGridRecord {
    fn from(r: &mako_markt::repository::MaloGridRecord) -> Self {
        Self {
            malo_id: r.malo_id.to_string(),
            nb_mp_id: r.nb_mp_id.clone(),
            bilanzierungsgebiet: r.bilanzierungsgebiet.clone(),
            netzgebiet: r.netzgebiet.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_helpers() {
        let accept = NbEntscheidung::accept(
            crate::codes::EBD_LIEFERBEGINN,
            crate::codes::lookup(crate::codes::EBD_LIEFERBEGINN, "A51").expect("A51"),
        );
        assert!(accept.is_accept());
        assert!(!accept.is_reject());
        assert!(!accept.is_escalate());
        // A Bestätigung states a code — the AHB marks SG4 STS+E01 Muss.
        assert_eq!(accept.antwortcode(), Some("A51"));
        assert_eq!(accept.ebd(), Some("E_0623"));

        let reject = NbEntscheidung::Reject(RejectReason::new(
            crate::codes::EBD_ANMELDUNG_DIREKT_ABLEHNBAR,
            crate::codes::lookup(crate::codes::EBD_ANMELDUNG_DIREKT_ABLEHNBAR, "A06").expect("A06"),
            70,
            "Conflicting supply",
        ));
        assert!(reject.is_reject());
        assert_eq!(reject.antwortcode(), Some("A06"));
        assert_eq!(reject.ebd(), Some("E_0622"));

        let escalate = NbEntscheidung::Escalate {
            reason: "Grid record missing".to_owned(),
        };
        assert!(escalate.is_escalate());
        assert!(escalate.antwortcode().is_none());
    }

    #[test]
    fn messtyp_display() {
        assert_eq!(Messtyp::Slp.to_string(), "SLP");
        assert_eq!(Messtyp::Rlm.to_string(), "RLM");
        assert_eq!(Messtyp::Imsys.to_string(), "IMSYS");
    }
}
