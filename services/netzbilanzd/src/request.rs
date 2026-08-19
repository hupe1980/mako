//! The billing request model — what an operator asks `netzbilanzd` to settle.
//!
//! # A tagged enum, not a bag of options
//!
//! The settlement kind is a `#[serde(tag = "billing_type")]` enum: each variant
//! carries exactly its own fields, `sparte` is required where it matters, and a
//! field that does not apply cannot be sent. The variants map one-to-one onto
//! the `grid-billing` entry points.
//!
//! A flat struct with a `billing_type: String` and one `Option` per field
//! admits three defect classes this shape rules out: fields accepted and
//! ignored (a `grundpreis` read only by one branch is a charge that silently
//! does not happen), a Sparte that cannot be stated per settlement, and an
//! unknown kind discovered at the match arm rather than the request boundary.
//!
//! # Round-trippable by design
//!
//! Every type here is `Serialize + Deserialize`, and [`SettlementRequest`] is
//! stored verbatim on the draft. That is what makes a Stornorechnung honest: the
//! reversal is `reverse(recompute(stored input))`, not a JSON edit of a rendered
//! document. The stored input is also the audit answer to "how was this figure
//! reached" — replayable, not merely described.

use grid_billing::{
    ArbeitspreisModell, AwhPositionInput, Blindarbeit, GasKapazitaet, Grundpreis,
    Konzessionsabgabe, Leistungspreis, MsbEmpfaengerRolle, Sparte,
};
use grid_billing::{msbg, netzebene, sect19, umlagen};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Request body for `POST /api/v1/billing/run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRunRequest {
    /// Invoice issue date.
    pub invoice_date: time::Date,
    /// Payment due date (Zahlungsziel, §271 BGB).
    pub due_date: time::Date,
    /// Optional Rechnungskreis — a short prefix for the generated invoice
    /// numbers (e.g. `"NNE"`). The running number itself is allocated by the
    /// database, never by the caller: §14 Abs. 4 Nr. 4 UStG requires an
    /// *einmalig vergebene* fortlaufende Nummer, and a caller-supplied prefix
    /// plus a per-request counter collides the moment the same run is repeated.
    #[serde(default)]
    pub rechnungskreis: Option<String>,
    /// Billing positions — one invoice per entry.
    pub positions: Vec<BillingPositionRequest>,
}

/// One MaLo to settle, and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingPositionRequest {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Start of the delivery period (inclusive).
    pub period_from: time::Date,
    /// End of the delivery period (inclusive).
    pub period_to: time::Date,
    /// What to settle, and with which inputs.
    pub settlement: SettlementRequest,
    /// The billing cadence — `IMD+7081` on the wire.
    ///
    /// A document fact, not a calculation one: the same settlement is the same
    /// arithmetic whether billed monthly, per Turnus, or as the Abschlussrechnung
    /// that closes a year. Left unset, the field is omitted rather than guessed.
    #[serde(default)]
    pub cadence: Option<grid_billing::Rechnungscharakter>,
    /// Draft IDs of the Abschlagsrechnungen this invoice settles.
    ///
    /// Each is looked up and deducted from what is owed — never from the net or
    /// the tax, because §14 Abs. 5 UStG already taxed the Anzahlung when it was
    /// received. The amount and the invoice number come from the stored draft
    /// rather than the request, so the deduction always matches what was
    /// actually billed (INVOIC AHB rule \[526\]), and a reversed Abschlag is
    /// refused rather than deducted (rule \[519\]).
    #[serde(default)]
    pub abschlaege: Vec<uuid::Uuid>,
}

/// Which settlement to run, with exactly the inputs that settlement takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "billing_type", rename_all = "snake_case")]
pub enum SettlementRequest {
    /// Abschlagsrechnung Netznutzung — a payment on account, PID 31001.
    Abschlag(AbschlagRequest),
    /// Netznutzungsentgelt — NN-Rechnung, PID 31002 for both Sparten.
    Nne(Box<NneRequest>),
    /// Mehr-/Mindermengensaldo — PID 31005.
    Mmm(MmmRequest),
    /// Messstellenbetrieb — PID 31009, issued **by** the MSB.
    Msb(MsbRequest),
    /// GeLi Gas abrechnungswürdige Handlungen — PID 31011.
    GasAwh(GasAwhRequest),
}

impl SettlementRequest {
    /// The Sparte this settlement is for.
    ///
    /// AWH exists only in the Gas Sperrprozess (BK7-24-01-009 §5.4), so the
    /// variant fixes it rather than asking.
    #[must_use]
    pub const fn sparte(&self) -> Sparte {
        match self {
            Self::Abschlag(a) => a.sparte,
            Self::Nne(n) => n.sparte,
            Self::Mmm(m) => m.sparte,
            // A Messstellenbetrieb charge is not Sparte-neutral, but §30 MsbG
            // prices metering, not energy — the Sparte rides on the metering
            // point, and the settlement carries no per-Sparte arithmetic.
            Self::Msb(m) => m.sparte,
            Self::GasAwh(_) => Sparte::Gas,
        }
    }
}

// ── Abschlag ──────────────────────────────────────────────────────────────────

/// Abschlagsrechnung inputs (`billing_type: "abschlag"`, PID 31001).
///
/// A payment on account: an amount asked for against a period that has not been
/// settled. There is no quantity and no Arbeitspreis, and the invoice carries
/// **exactly one** Positionszeile (INVOIC AHB 1.0b, Änd-ID 26817).
///
/// The Abschlussrechnung that follows deducts it by invoice number — see
/// [`BillingPositionRequest::abschlaege`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbschlagRequest {
    /// Netzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — the invoice recipient.
    pub lf_mp_id: String,
    /// `"Strom"` or `"Gas"`.
    pub sparte: Sparte,
    /// The **net** amount requested, in EUR. The tax is stated separately.
    pub betrag_netto_eur: Decimal,
    /// How the amount was arrived at — recorded for the audit, not computed.
    pub grundlage: grid_billing::AbschlagGrundlage,
}

// ── NNE ───────────────────────────────────────────────────────────────────────

/// Netznutzungsentgelt inputs (`billing_type: "nne"`).
///
/// Mirrors `grid_billing::NneInput` minus the identity and period the enclosing
/// [`BillingPositionRequest`] already carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NneRequest {
    /// Netzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — the invoice recipient.
    pub lf_mp_id: String,
    /// `"STROM"` or `"GAS"`. Required: it selects StromNEV §21 vs GasNEV §14,
    /// the `SettlementType`, and whether the three EnFG network levies apply at
    /// all. There is no safe default — defaulting to `Strom` would put
    /// ~2.95 ct/kWh of electricity levies on every gas invoice.
    pub sparte: Sparte,
    /// How the Arbeitspreis is structured — flat, or one of the three §14a
    /// modules (BK6-22-300 / BK8-22/010-A), or a spot-linked Netzentgelt.
    pub arbeitspreis: ArbeitspreisModell,
    /// RLM demand charge — peak and rate together, or neither.
    #[serde(default)]
    pub leistungspreis: Option<Leistungspreis>,
    /// Gas Verrechnungspreis (§14 GasNEV) — monthly rate and months billed.
    #[serde(default)]
    pub grundpreis: Option<Grundpreis>,
    /// Konzessionsabgabe — rate **and** KAV §2 customer group, so the
    /// Höchstbetrag can be checked. A bare rate cannot be checked against
    /// anything, which is exactly when an over-charge goes unnoticed.
    #[serde(default)]
    pub konzessionsabgabe: Option<Konzessionsabgabe>,
    /// Blindmehrarbeit — reactive energy beyond the price sheet's free share.
    #[serde(default)]
    pub blindarbeit: Option<Blindarbeit>,
    /// Gas Kapazitätsentgelt (§15 GasNEV).
    #[serde(default)]
    pub gas_kapazitaet: Option<GasKapazitaet>,
    /// EnFG Letztverbrauchergruppe for the network levies. Defaults to `A` —
    /// the full levy — because that is what applies absent a granted privilege.
    #[serde(default)]
    pub letztverbrauchergruppe: umlagen::Letztverbrauchergruppe,
    /// Override the tabled §19 StromNEV rate for the delivery year, in ct/kWh.
    #[serde(default)]
    pub sect19_umlage_ct_per_kwh: Option<Decimal>,
    /// Override the tabled Offshore-Netzumlage, in ct/kWh.
    #[serde(default)]
    pub offshore_umlage_ct_per_kwh: Option<Decimal>,
    /// Override the tabled KWKG-Umlage, in ct/kWh.
    #[serde(default)]
    pub kwkg_umlage_ct_per_kwh: Option<Decimal>,
    /// The Netzebene the rate was published for — recorded, and used for the
    /// §17 Abs. 6 StromNEV Arbeitspreis-only check.
    #[serde(default)]
    pub netzebene: Option<netzebene::Netzebene>,
    /// An agreed §19 Abs. 2 individual charge.
    #[serde(default)]
    pub sect19: Option<sect19::Sect19Vereinbarung>,
    /// Annual peak demand in kW, for the Benutzungsstundenzahl.
    #[serde(default)]
    pub jahreshoechstleistung_kw: Option<Decimal>,
    /// Annual energy in kWh, for the Benutzungsstundenzahl and the §17 Abs. 6 check.
    #[serde(default)]
    pub jahresarbeit_kwh: Option<Decimal>,
    /// Price-sheet identifier, carried into every position's trace.
    #[serde(default)]
    pub tariff_sheet_id: Option<String>,
}

// ── MMM ───────────────────────────────────────────────────────────────────────

/// Mehr-/Mindermengensaldo inputs (`billing_type: "mmm"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmmRequest {
    /// Netzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — the invoice recipient.
    pub lf_mp_id: String,
    /// `"STROM"` (GPKE BK6-24-174 Teil 1 Kap. 8.4) or `"GAS"` (GaBi Gas 2.1,
    /// BK7-24-01-008). It selects the legal references *and* which published
    /// price series is auto-fetched, so it cannot be inferred.
    pub sparte: Sparte,
    /// The measured quantity for the period, in kWh.
    pub gemessen_kwh: Decimal,
    /// The bilanzierte (profile-allocated) quantity for the period, in kWh.
    pub bilanziert_kwh: Decimal,
    /// Mehrmengen price in ct/kWh. Auto-fetched from `marktd` when absent —
    /// Trading Hub Europe for Gas, the configured ÜNB's series for Strom.
    #[serde(default)]
    pub mehr_preis_ct_per_kwh: Option<Decimal>,
    /// Mindermengen price in ct/kWh. Auto-fetched alongside the Mehrmengen price.
    #[serde(default)]
    pub minder_preis_ct_per_kwh: Option<Decimal>,
    /// SLP Lastprofil (`"H0"`, `"G0"`…). Auto-derived from the MaLo's
    /// `bilanzierungsmethode` in `marktd` when absent.
    #[serde(default)]
    pub lastprofil: Option<String>,
    /// Who holds §3g Wiederverkäufer status, evidenced by a valid *USt 1 TH*.
    ///
    /// A Mehr-/Mindermenge is a **Lieferung** of the commodity, not a network
    /// service, so §13b Abs. 2 Nr. 5 Buchst. b UStG can shift the tax to the
    /// recipient. The condition is asymmetric — electricity needs both parties,
    /// gas needs the recipient — and getting it wrong is a §14c Abs. 1 liability
    /// rather than a rounding error, so both facts are stated rather than
    /// inferred. Defaults to neither party holding it, which is the taxed case.
    #[serde(default)]
    pub wiederverkaeufer: grid_billing::Wiederverkaeuferstatus,
}

// ── MSB ───────────────────────────────────────────────────────────────────────

/// Messstellenbetrieb inputs (`billing_type: "msb"`, PID 31009).
///
/// The MSB is the sender in all seven Anwendungsfälle of the PID overview 4.0;
/// it is never the recipient.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsbRequest {
    /// Messstellenbetreiber MP-ID — the invoice **sender**.
    pub msb_mp_id: String,
    /// Which market role is billed.
    pub empfaenger_rolle: MsbEmpfaengerRolle,
    /// MP-ID of the billed party.
    pub empfaenger_mp_id: String,
    /// Sparte of the metering point.
    pub sparte: Sparte,
    /// Grundgebühr Messstellenbetrieb in EUR per month (`PreisblattMessung`).
    pub grundgebuehr_eur_per_month: Decimal,
    /// Full calendar months in the billing period.
    pub billing_months: u32,
    /// Optional Messdienstleistung flat fee in EUR for the whole period.
    #[serde(default)]
    pub messdienstleistung_eur: Option<Decimal>,
    /// Which §30 MsbG case the metering point falls under. Supplying it turns
    /// on the Preisobergrenze check — a charge above the POG is an amount the
    /// customer is entitled to have refunded.
    #[serde(default)]
    pub messstellen_kategorie: Option<msbg::MessstellenKategorie>,
    /// Whose share of the §30 MsbG ceiling this settlement bills.
    #[serde(default)]
    pub entgeltschuldner: Option<msbg::Entgeltschuldner>,
}

// ── Gas AWH ───────────────────────────────────────────────────────────────────

/// GeLi Gas abrechnungswürdige Handlungen (`billing_type: "gas_awh"`, PID 31011).
///
/// AWH are **per-action** charges — a Sperrung executed, a Wiederherstellung
/// outside regular hours — priced in EUR per execution from the price sheet.
/// They are not energy, which is why this is `settle_gas_awh` and not an NNE
/// settlement with a different Prüfidentifikator: billing them per kWh under
/// StromNEV §21 would cite the wrong ordinance and the wrong Sparte, and add
/// electricity levies to a gas Sperrprozess invoice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasAwhRequest {
    /// Gasnetzbetreiber MP-ID — the invoice sender.
    pub nb_mp_id: String,
    /// Lieferant Gas MP-ID (LFG/LFA) — the invoice recipient.
    pub lf_mp_id: String,
    /// The chargeable actions. At least one is required.
    pub positionen: Vec<AwhPositionInput>,
    /// Price-sheet identifier, carried into every position's trace.
    #[serde(default)]
    pub tariff_sheet_id: Option<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    fn parse(body: serde_json::Value) -> Result<BillingPositionRequest, serde_json::Error> {
        serde_json::from_value(body)
    }

    fn nne_position(settlement: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "malo_id": "51238696012",
            "period_from": "2026-01-01",
            "period_to": "2026-01-31",
            "settlement": settlement,
        })
    }

    /// A Gas NNE states its Sparte, and it survives into the request.
    #[test]
    fn a_gas_nne_position_carries_its_sparte() {
        let pos = parse(nne_position(serde_json::json!({
            "billing_type": "nne",
            "nb_mp_id": "9900357000004",
            "lf_mp_id": "9900012345678",
            "sparte": "Gas",
            "arbeitspreis": { "Einheitlich": { "menge_kwh": "3000", "preis_ct_per_kwh": "1.80" } }
        })))
        .expect("a gas NNE position parses");
        assert_eq!(pos.settlement.sparte(), Sparte::Gas);
    }

    /// A field that belongs to another settlement kind is refused, not ignored.
    ///
    /// An accepted-and-dropped `grundgebuehr_eur_per_month` on an NNE position
    /// is a Grundpreis that goes unbilled.
    #[test]
    fn a_field_from_another_settlement_kind_is_refused() {
        let outcome = parse(nne_position(serde_json::json!({
            "billing_type": "nne",
            "nb_mp_id": "9900357000004",
            "lf_mp_id": "9900012345678",
            "sparte": "Strom",
            "arbeitspreis": { "Einheitlich": { "menge_kwh": "1000", "preis_ct_per_kwh": "3.5" } },
            "grundgebuehr_eur_per_month": "9.50"
        })));
        assert!(
            outcome.is_err(),
            "an MSB field on an NNE position must not be silently dropped"
        );
    }

    /// An unknown `billing_type` fails at the request boundary, not in a match arm.
    ///
    /// The Sparte is a field, so Sparte-suffixed kind names are not valid
    /// `billing_type`s and must not parse as one.
    #[test]
    fn an_unknown_billing_type_is_a_parse_error() {
        for unknown in ["nne_strom", "mmm_gas", "typo"] {
            assert!(
                parse(nne_position(serde_json::json!({ "billing_type": unknown }))).is_err(),
                "{unknown} must not parse"
            );
        }
    }

    /// §14a Modul 3 arrives as all three bands or not at all.
    #[test]
    fn modul_3_requires_all_three_bands() {
        let ok = parse(nne_position(serde_json::json!({
            "billing_type": "nne",
            "nb_mp_id": "9900357000004",
            "lf_mp_id": "9900012345678",
            "sparte": "Strom",
            "arbeitspreis": { "Modul3ZeitVariabel": {
                "ht": { "menge_kwh": "600", "preis_ct_per_kwh": "4.20" },
                "st": { "menge_kwh": "100", "preis_ct_per_kwh": "3.00" },
                "nt": { "menge_kwh": "400", "preis_ct_per_kwh": "1.50" }
            }}
        })))
        .expect("all three bands parse");
        let SettlementRequest::Nne(nne) = ok.settlement else {
            panic!("expected an NNE settlement");
        };
        assert_eq!(nne.arbeitspreis.menge_kwh(), dec!(1100));

        assert!(
            parse(nne_position(serde_json::json!({
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Modul3ZeitVariabel": {
                    "ht": { "menge_kwh": "600", "preis_ct_per_kwh": "4.20" },
                    "nt": { "menge_kwh": "400", "preis_ct_per_kwh": "1.50" }
                }}
            })))
            .is_err(),
            "a missing Standardtarif band must be refused, not defaulted to zero"
        );
    }

    /// A §14a Modul 2 factor outside (0, 1] is refused at the boundary.
    #[test]
    fn an_out_of_range_modul_2_factor_is_refused() {
        assert!(
            parse(nne_position(serde_json::json!({
                "billing_type": "nne",
                "nb_mp_id": "9900357000004",
                "lf_mp_id": "9900012345678",
                "sparte": "Strom",
                "arbeitspreis": { "Modul2ProzentualeReduzierung": {
                    "basis": { "menge_kwh": "1000", "preis_ct_per_kwh": "3.5" },
                    "reduktion": "5"
                }}
            })))
            .is_err(),
            "a factor of 5 would multiply the Arbeitspreis, not reduce it"
        );
    }

    /// The whole settlement input round-trips, which is what makes a
    /// Stornorechnung a recomputation rather than a JSON edit.
    #[test]
    fn a_settlement_request_round_trips() {
        let pos = parse(nne_position(serde_json::json!({
            "billing_type": "gas_awh",
            "nb_mp_id": "9900357000004",
            "lf_mp_id": "9900012345678",
            "positionen": [{
                "beschreibung": "Sperrung Gaszähler",
                "anzahl": 1,
                "preis_eur": "45.00",
                "artikel_id": "2-01-7-001"
            }]
        })))
        .expect("parse");
        let round_tripped = serde_json::to_value(&pos).expect("serialize");
        let again: BillingPositionRequest =
            serde_json::from_value(round_tripped).expect("re-parse");
        assert_eq!(again.settlement.sparte(), Sparte::Gas);
        assert_eq!(again.malo_id, "51238696012");
    }
}
