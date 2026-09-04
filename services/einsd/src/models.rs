//! The settlement-model vocabulary — one token per model, named once.
//!
//! `eeg_anlagen.settlement_model` accepts exactly one token per model: no German
//! and English spellings of the same thing, so a gate cannot apply to one and
//! miss the other. These constants are the single source for it — [`ALL`] is
//! asserted equal to the schema's `CHECK` list by `tests/schema_code_guard.rs`,
//! so a token added to one and not the other fails the build rather than a
//! settlement.

/// §21 Abs. 1 EEG 2023 — Einspeisevergütung.
pub const VERGUETUNG: &str = "VERGUETUNG";
/// §21 Abs. 1 Satz 1 Nr. 3 EEG 2023 — Ausfallvergütung (Einspeisevergütung −20 % nach §53 Abs. 3).
/// It was Nr. 2 under the EEG 2017; the EEG 2023 inserted the unentgeltliche
/// Abnahme in front of it.
pub const AUSFALLVERGUETUNG: &str = "AUSFALLVERGUETUNG";
/// §20 EEG 2023 — gleitende Marktprämie (Direktvermarktung).
pub const DIREKTVERMARKTUNG: &str = "DIREKTVERMARKTUNG";
/// §22 EEG 2023 — wettbewerblich ermittelte Marktprämie (Ausschreibung).
pub const AUSSCHREIBUNG: &str = "AUSSCHREIBUNG";
/// §21a EEG 2023 — sonstige Direktvermarktung; no EEG payment from the NB.
pub const SONSTIGE_DIREKTVERMARKTUNG: &str = "SONSTIGE_DIREKTVERMARKTUNG";
/// §21 Abs. 3 EEG 2023 — Mieterstromzuschlag.
pub const MIETERSTROM: &str = "MIETERSTROM";
/// §42b EnWG — gemeinschaftliche Gebäudeversorgung.
pub const GGV: &str = "GGV";
/// Self-consumption only — no grid feed-in, no payment.
pub const EIGENVERBRAUCH: &str = "EIGENVERBRAUCH";
/// After the Förderdauer: the plant is paid the market value.
pub const POST_EEG_SPOT: &str = "POST_EEG_SPOT";
/// §7 KWKG 2023 — KWK-Zuschlag.
pub const KWKG_ZUSCHLAG: &str = "KWKG_ZUSCHLAG";
/// §50b EEG 2023 — Flexibilitätsprämie (Bestandsanlagen), ct/kWh.
pub const FLEXIBILITAET: &str = "FLEXIBILITAET";
/// §50a EEG 2023 — Flexibilitätszuschlag (Neuanlagen), EUR/kW/year.
pub const FLEXIBILITAET_ZUSCHLAG: &str = "FLEXIBILITAET_ZUSCHLAG";

/// Every accepted token, in the schema's order.
pub const ALL: [&str; 12] = [
    VERGUETUNG,
    AUSFALLVERGUETUNG,
    DIREKTVERMARKTUNG,
    AUSSCHREIBUNG,
    SONSTIGE_DIREKTVERMARKTUNG,
    MIETERSTROM,
    GGV,
    EIGENVERBRAUCH,
    POST_EEG_SPOT,
    KWKG_ZUSCHLAG,
    FLEXIBILITAET,
    FLEXIBILITAET_ZUSCHLAG,
];

/// The models settled as a Direktvermarktung, i.e. paid a Marktprämie.
///
/// These emit `de.eeg.marktpraemie.berechnet` and resolve their market reference
/// through the Anlage 1 Marktwert rather than the generic EPEX average.
#[must_use]
pub fn ist_marktpraemie(model: &str) -> bool {
    matches!(model, DIREKTVERMARKTUNG | AUSSCHREIBUNG)
}

/// Whether `model` is one this service knows how to settle.
#[must_use]
pub fn is_known(model: &str) -> bool {
    ALL.contains(&model)
}
