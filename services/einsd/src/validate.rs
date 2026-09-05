//! What a plant registration has to state before it can be settled.
//!
//! `POST /api/v1/anlagen` refuses a registration the settlement could not act on
//! — above all a Marktprämie model with no anzulegender Wert, which would settle
//! to `max(0, 0 − Marktwert)` = EUR 0 every month and be indistinguishable from a
//! month in which nothing was owed.
//!
//! The engine refuses the same thing (`PriceMissing`). Both layers are
//! deliberate: the engine is the last line for a row written straight to the
//! database, and the API is where an operator can still fix it.

use rust_decimal::Decimal;

use crate::models;
use crate::pg::AnlageUpsertRequest;

/// Reject a registration the settlement could not honestly act on.
///
/// # Errors
/// Returns a message naming the field and the rule it broke.
pub fn check(req: &AnlageUpsertRequest) -> Result<(), String> {
    if req.tr_id.trim().is_empty() {
        return Err("tr_id must not be empty".to_owned());
    }
    if req.malo_id.trim().is_empty() {
        return Err("malo_id must not be empty".to_owned());
    }

    // A capacity of zero or less is not a small plant. It drives the §9 band, the
    // §52 Pflichtzahlung (10 €/kW — negative capacity would *credit* the
    // operator), the §44b quota and the §51 exemption, and every one of them
    // silently produces nonsense from it.
    if req.leistung_kwp <= Decimal::ZERO {
        return Err(format!(
            "leistung_kwp must be positive, got {}",
            req.leistung_kwp
        ));
    }
    if req.verguetungssatz_ct < Decimal::ZERO {
        return Err(format!(
            "verguetungssatz_ct must not be negative, got {}",
            req.verguetungssatz_ct
        ));
    }

    if !models::is_known(&req.settlement_model) {
        return Err(format!(
            "unknown settlement_model `{}` — expected one of {}",
            req.settlement_model,
            models::ALL.join(", ")
        ));
    }

    // ── The fields each model cannot be settled without ──────────────────────
    match req.settlement_model.as_str() {
        models::DIREKTVERMARKTUNG => {
            if req.direktverm_aw_ct.is_none_or(|aw| aw <= Decimal::ZERO) {
                return Err(
                    "DIREKTVERMARKTUNG needs a positive direktverm_aw_ct: the Marktprämie \
                     is max(0, AW − Marktwert), so without an anzulegender Wert every \
                     month settles to EUR 0"
                        .to_owned(),
                );
            }
        }
        models::AUSSCHREIBUNG => {
            // Either column carries the AW for a tender plant — `zuschlagswert_ct`
            // is the awarded value and takes precedence at settlement.
            let aw = req.zuschlagswert_ct.or(req.direktverm_aw_ct);
            if aw.is_none_or(|aw| aw <= Decimal::ZERO) {
                return Err(
                    "AUSSCHREIBUNG needs a positive zuschlagswert_ct (the awarded \
                     anzulegender Wert) or direktverm_aw_ct — without one the \
                     Marktprämie is max(0, 0 − Marktwert) = EUR 0 every month"
                        .to_owned(),
                );
            }
            if req
                .ausschreibungs_zuschlag_id
                .as_ref()
                .is_none_or(|z| z.trim().is_empty())
            {
                return Err(
                    "AUSSCHREIBUNG needs the ausschreibungs_zuschlag_id: an awarded AW \
                     without the award it came from cannot be audited (§22 EEG 2023)"
                        .to_owned(),
                );
            }
        }
        models::MIETERSTROM => {
            if req.mieter_zuschlag_ct.is_none() {
                return Err(
                    "MIETERSTROM needs mieter_zuschlag_ct (§21 Abs. 3 EEG 2023) — \
                     omitting it settles the plant as a plain Einspeisevergütung"
                        .to_owned(),
                );
            }
        }
        models::KWKG_ZUSCHLAG => {
            if req.kwk_foerderdauer_h.is_none() && req.kwk_anlagenart.is_none() {
                return Err("KWKG_ZUSCHLAG needs kwk_foerderdauer_h (§8 Abs. 1–3 \
                     Vollbenutzungsstunden) or kwk_anlagenart to derive them from — \
                     §8 limits the Zuschlag in Vollbenutzungsstunden, and a plant \
                     with neither is never exhausted"
                    .to_owned());
            }
            if req.kwk_anlagenart.as_deref() != Some("NEU")
                && req.kwk_foerderdauer_h.is_none()
                && req.kwk_kostenanteil.is_none()
            {
                return Err(
                    "a modernisierte or nachgerüstete KWK-Anlage needs kwk_kostenanteil — \
                     §8 Abs. 2 and Abs. 3 key the Vollbenutzungsstunden on the share of \
                     the Neuerrichtungskosten the work cost"
                        .to_owned(),
                );
            }
        }
        models::FLEXIBILITAET => {
            if req.flex_praemie_ct_kwh.is_none() {
                return Err("FLEXIBILITAET needs flex_praemie_ct_kwh (§50b EEG 2023)".to_owned());
            }
        }
        _ => {}
    }

    // ── Technology and statute must agree ────────────────────────────────────
    let ist_kwkg_technologie = req.erzeugungsart == "KWKG";
    let ist_kwkg_modell = req.settlement_model == models::KWKG_ZUSCHLAG;
    if ist_kwkg_technologie != ist_kwkg_modell {
        return Err(format!(
            "erzeugungsart `{}` and settlement_model `{}` disagree: the KWKG is a \
             different statute from the EEG, and a plant on one cannot be settled \
             under the other",
            req.erzeugungsart, req.settlement_model
        ));
    }
    if ist_kwkg_technologie && req.eeg_gesetz != 0 {
        return Err(format!(
            "a KWKG plant carries eeg_gesetz = 0, got {}",
            req.eeg_gesetz
        ));
    }
    if !ist_kwkg_technologie && req.eeg_gesetz == 0 {
        return Err(
            "eeg_gesetz = 0 marks a KWKG plant; an EEG plant carries its law year".to_owned(),
        );
    }

    // The rate table is keyed on the Vergütungsform, so a mismatch finds no rate.
    let kwk_form = req.verguetungsform == "KWK_ZUSCHLAG";
    if kwk_form != ist_kwkg_technologie {
        return Err(format!(
            "verguetungsform `{}` does not belong to erzeugungsart `{}`",
            req.verguetungsform, req.erzeugungsart
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    fn solar() -> AnlageUpsertRequest {
        serde_json::from_value(serde_json::json!({
            "tr_id": "TR-1",
            "malo_id": "51238696781",
            "eeg_gesetz": 2023,
            "inbetriebnahme": "2024-06-01",
            "leistung_kwp": "9.5",
            "erzeugungsart": "SOLAR_AUFDACH",
            "verguetungssatz_ct": "8.11",
            "settlement_model": "VERGUETUNG",
            "einspeiser_id": "EB-1",
        }))
        .expect("fixture parses")
    }

    #[test]
    fn a_plain_solar_plant_is_accepted() {
        assert!(check(&solar()).is_ok());
    }

    /// The one that mattered: every month would have settled to EUR 0 and emitted
    /// a payout event for it.
    #[test]
    fn direktvermarktung_without_an_aw_is_refused() {
        let req = AnlageUpsertRequest {
            settlement_model: models::DIREKTVERMARKTUNG.to_owned(),
            direktverm_aw_ct: None,
            ..solar()
        };
        let err = check(&req).expect_err("must be refused");
        assert!(err.contains("direktverm_aw_ct"), "{err}");

        let ok = AnlageUpsertRequest {
            direktverm_aw_ct: Some(dec!(7.0)),
            ..req
        };
        assert!(check(&ok).is_ok());
    }

    /// A tender plant may carry its AW in either column — `zuschlagswert_ct` is
    /// the one an operator reaches for, and the settlement now prefers it.
    #[test]
    fn ausschreibung_accepts_the_awarded_value() {
        let base = AnlageUpsertRequest {
            settlement_model: models::AUSSCHREIBUNG.to_owned(),
            ausschreibungs_zuschlag_id: Some("SEE-2024-001234".to_owned()),
            direktverm_aw_ct: None,
            ..solar()
        };
        assert!(check(&base).is_err(), "neither column set");
        assert!(
            check(&AnlageUpsertRequest {
                zuschlagswert_ct: Some(dec!(7.35)),
                ..base
            })
            .is_ok()
        );
    }

    #[test]
    fn ausschreibung_needs_its_award_reference() {
        let req = AnlageUpsertRequest {
            settlement_model: models::AUSSCHREIBUNG.to_owned(),
            zuschlagswert_ct: Some(dec!(7.35)),
            ..solar()
        };
        let err = check(&req).expect_err("must be refused");
        assert!(err.contains("ausschreibungs_zuschlag_id"), "{err}");
    }

    #[test]
    fn a_non_positive_capacity_is_refused() {
        for kwp in [dec!(0), dec!(-5)] {
            let req = AnlageUpsertRequest {
                leistung_kwp: kwp,
                ..solar()
            };
            assert!(check(&req).is_err(), "{kwp} must be refused");
        }
    }

    #[test]
    fn the_statutes_must_not_be_mixed() {
        // A solar plant on the KWKG model.
        let req = AnlageUpsertRequest {
            settlement_model: models::KWKG_ZUSCHLAG.to_owned(),
            kwk_anlagenart: Some("NEU".to_owned()),
            ..solar()
        };
        assert!(check(&req).is_err());

        // A KWK plant that forgot its law-year marker.
        let req = AnlageUpsertRequest {
            erzeugungsart: "KWKG".to_owned(),
            settlement_model: models::KWKG_ZUSCHLAG.to_owned(),
            verguetungsform: "KWK_ZUSCHLAG".to_owned(),
            kwk_anlagenart: Some("NEU".to_owned()),
            eeg_gesetz: 2023,
            ..solar()
        };
        assert!(check(&req).is_err());

        // Stated coherently, it is accepted.
        let req = AnlageUpsertRequest {
            erzeugungsart: "KWKG".to_owned(),
            settlement_model: models::KWKG_ZUSCHLAG.to_owned(),
            verguetungsform: "KWK_ZUSCHLAG".to_owned(),
            kwk_anlagenart: Some("NEU".to_owned()),
            eeg_gesetz: 0,
            ..solar()
        };
        assert!(check(&req).is_ok(), "{:?}", check(&req));
    }

    #[test]
    fn a_kwk_plant_without_a_limit_is_refused() {
        let req = AnlageUpsertRequest {
            erzeugungsart: "KWKG".to_owned(),
            settlement_model: models::KWKG_ZUSCHLAG.to_owned(),
            verguetungsform: "KWK_ZUSCHLAG".to_owned(),
            eeg_gesetz: 0,
            ..solar()
        };
        let err = check(&req).expect_err("must be refused");
        assert!(err.contains("kwk_foerderdauer"), "{err}");
    }

    #[test]
    fn mieterstrom_needs_its_zuschlag() {
        let req = AnlageUpsertRequest {
            settlement_model: models::MIETERSTROM.to_owned(),
            ..solar()
        };
        assert!(check(&req).is_err());
    }
}
