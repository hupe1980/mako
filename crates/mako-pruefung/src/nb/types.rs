//! Inputs and outputs of the Netzbetreiber's `E_0622` / `E_0607` decisions.
//!
//! All types are `Clone + Debug + Serialize + Deserialize` so that callers can
//! log inputs/outputs and store audit records without extra conversions.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use mako_markt::domain::Sparte;

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
}

/// A resolved Antwortcode: the code, the tree that publishes it, and the BDEW's
/// own wording.
///
/// Built only from an [`AntwortCode`] in [`crate::codes`], so a code can never
/// travel under a tree that does not define it: `G_0011` answers with
/// `A16` / `E13` / `ZC5` where `E_0622` answers `A02` / `A05` / `A06`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntwortDetail {
    /// The EBD id the code was **resolved against** — always present, and the
    /// key [`crate::codes::lookup`] takes.
    ///
    /// Distinct from [`Self::ebd`]: the Gas Codelisten are not named in `STS`
    /// DE 1131, so a Gas answer carries no EBD *on the wire* while still
    /// belonging to exactly one tree.
    pub tree: String,
    /// `SG4 STS+E01` DE 9013 — the Antwortcode.
    ///
    /// Not an ERC code: `ERC` is the APERAK/CONTRL processability segment.
    pub antwortcode: String,
    /// `SG4 STS+E01` DE 1131 — the EBD that publishes the code (`"E_0622"`,
    /// `"E_0607"`), or `None` on the Gas Codelisten, which the MIG does not
    /// require to be named.
    pub ebd: Option<String>,
    /// The BDEW's own wording for the code, for the operator queue and the
    /// § 20 EnWG audit log.
    pub bedeutung: String,
    /// `true` when the BDEW requires a written Erläuterung (`FTX+ACB`)
    /// alongside the code. Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
}

impl AntwortDetail {
    /// Resolve a published code into its wire form, naming the tree it came from.
    #[must_use]
    pub fn new(tree: &'static str, code: &'static AntwortCode) -> Self {
        Self {
            tree: tree.to_owned(),
            antwortcode: code.code.to_owned(),
            ebd: code.ebd.map(ToOwned::to_owned),
            bedeutung: code.bedeutung.to_owned(),
            braucht_bemerkung: code.braucht_bemerkung,
        }
    }
}

/// A structured rejection: the resolved BDEW **Antwortcode**, the EBD
/// Prüfschritt that produced it, and a human-readable explanation for the
/// BNetzA audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectReason {
    /// The resolved Antwortcode.
    #[serde(flatten)]
    pub antwort: AntwortDetail,
    /// Human-readable explanation for the operator and the § 20 EnWG audit log.
    ///
    /// When [`AntwortDetail::braucht_bemerkung`] is set this is also what goes
    /// into `FTX+ACB` on the wire.
    pub detail: String,
    /// The **EBD Prüfschritt** number that produced the code — `15`, `270`,
    /// `806`. Not an ordinal of mako's own checks: an auditor holding the
    /// published tree can find the row this decision came from.
    pub pruefschritt: u16,
}

impl RejectReason {
    /// Build a rejection from a published Ablehnungscode.
    ///
    /// # Panics
    ///
    /// In debug builds, when `code` is a Zustimmung — that would put an
    /// agreement code on an Ablehnungs-PID.
    #[must_use]
    pub fn new(
        tree: &'static str,
        code: &'static AntwortCode,
        pruefschritt: u16,
        detail: impl Into<String>,
    ) -> Self {
        debug_assert_eq!(
            code.cluster,
            Cluster::Ablehnung,
            "{} is a Zustimmungscode and cannot carry a rejection",
            code.code
        );
        Self {
            antwort: AntwortDetail::new(tree, code),
            detail: detail.into(),
            pruefschritt,
        }
    }
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
            Self::Escalate { .. } => None,
        }
    }

    /// The EBD the Antwortcode belongs to.
    #[must_use]
    pub fn ebd(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => a.ebd.as_deref(),
            Self::Reject(r) => r.antwort.ebd.as_deref(),
            Self::Escalate { .. } => None,
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
