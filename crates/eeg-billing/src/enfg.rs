//! Which EnFG Belastungsausgleich head a settlement model's payment lands under.
//!
//! A Netzbetreiber that pays an EEG or KWKG plant is reimbursed by its
//! Übertragungsnetzbetreiber: § 13 Abs. 1 Satz 1 EnFG reads „Die
//! Übertragungsnetzbetreiber müssen ihren nachgelagerten Verteilernetzbetreibern
//! erstatten: 1. die nach § 6 Absatz 5, § 19, § 38d oder § 50 des
//! Erneuerbare-Energien-Gesetzes geleisteten Zahlungen … 2. die nach Maßgabe des
//! Abschnitts 2 des Kraft-Wärme-Kopplungsgesetzes geleisteten Zahlungen und
//! 3. die geleisteten Zahlungen … nach den Bestimmungen früherer Fassungen …".
//! The § 50 Nr. 1 Buchst. a EnFG Mitteilung asks for the same payments split
//! by what they paid for — aa „Strom aus erneuerbaren Energien und Grubengas
//! nach § 19 Absatz 1", bb „die Bereitstellung von installierter Leistung nach
//! § 50", cc „KWK-Strom". [`EnfgPosten`] is that split.
//!
//! The ÜNB's claim the other way is § 14 Satz 1 EnFG; its Nr. 3 is „Zahlungen
//! nach § 52 und § 55b des Erneuerbare-Energien-Gesetzes", so a § 52
//! Pflichtzahlung the Netzbetreiber collects is owed upward, never netted into
//! the plant's reimbursement.
//!
//! ## Nr. 1 and Nr. 3 are one head here
//!
//! A payment under an earlier EEG Fassung is § 13 Abs. 1 Satz 1 Nr. 3 rather
//! than Nr. 1, but neither consumer of the head separates them: § 13 Abs. 3
//! EnFG nets „Die Zahlungen nach Absatz 1 Satz 1 Nummer 1 und 3, soweit sie das
//! Erneuerbare-Energien-Gesetz betreffen" together, and § 50 Nr. 1 Buchst. a aa
//! reports § 19 Abs. 1 payments „oder nach den Bestimmungen früherer Fassungen
//! …, die § 19 Absatz 1 … entsprechen" as one item (cc does the same for the
//! KWKG). The plant's vintage stays on the plant row for the per-Anlage
//! Endabrechnung.
//!
//! Source: gesetze-im-internet.de `enfg/__13.html`, `__14.html`, `__50.html`,
//! read 2026-11-08; EEG 2023 §§ 19, 21, 21b, 23b, 50, 50a, 50b, 53 from the
//! Clearingstelle Arbeitsausgabe (Stand 23.12.2025).

/// The twelve settlement models a plant can be settled under — `einsd`'s
/// `eeg_anlagen.settlement_model` vocabulary, one variant per token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettlementModel {
    /// § 19 Abs. 1 Nr. 2 i.V.m. § 21 Abs. 1 Satz 1 EEG 2023 — Einspeisevergütung.
    Verguetung,
    /// § 21 Abs. 1 Satz 1 Nr. 3 EEG 2023 — Ausfallvergütung.
    Ausfallverguetung,
    /// § 19 Abs. 1 Nr. 1 i.V.m. § 20 EEG 2023 — Marktprämie.
    Direktvermarktung,
    /// § 22 EEG 2023 — Marktprämie with a tender-awarded anzulegender Wert.
    Ausschreibung,
    /// § 21a EEG 2023 — sonstige Direktvermarktung, no Netzbetreiber payment.
    SonstigeDirektvermarktung,
    /// § 19 Abs. 1 Nr. 3 i.V.m. § 21 Abs. 3 EEG 2023 — Mieterstromzuschlag.
    Mieterstrom,
    /// § 42b EnWG — gemeinschaftliche Gebäudeversorgung.
    Ggv,
    /// No grid feed-in, no payment.
    Eigenverbrauch,
    /// After the Förderdauer, settled on the market value.
    PostEegSpot,
    /// § 7 KWKG — KWK-Zuschlag.
    KwkgZuschlag,
    /// § 50b EEG 2023 — Flexibilitätsprämie, paid on top of the § 19 claim.
    Flexibilitaet,
    /// § 50a EEG 2023 — Flexibilitätszuschlag.
    FlexibilitaetZuschlag,
}

impl SettlementModel {
    /// Every model, in `einsd`'s schema order.
    pub const ALL: [Self; 12] = [
        Self::Verguetung,
        Self::Ausfallverguetung,
        Self::Direktvermarktung,
        Self::Ausschreibung,
        Self::SonstigeDirektvermarktung,
        Self::Mieterstrom,
        Self::Ggv,
        Self::Eigenverbrauch,
        Self::PostEegSpot,
        Self::KwkgZuschlag,
        Self::Flexibilitaet,
        Self::FlexibilitaetZuschlag,
    ];

    /// The storage token — `eeg_anlagen.settlement_model`.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Verguetung => "VERGUETUNG",
            Self::Ausfallverguetung => "AUSFALLVERGUETUNG",
            Self::Direktvermarktung => "DIREKTVERMARKTUNG",
            Self::Ausschreibung => "AUSSCHREIBUNG",
            Self::SonstigeDirektvermarktung => "SONSTIGE_DIREKTVERMARKTUNG",
            Self::Mieterstrom => "MIETERSTROM",
            Self::Ggv => "GGV",
            Self::Eigenverbrauch => "EIGENVERBRAUCH",
            Self::PostEegSpot => "POST_EEG_SPOT",
            Self::KwkgZuschlag => "KWKG_ZUSCHLAG",
            Self::Flexibilitaet => "FLEXIBILITAET",
            Self::FlexibilitaetZuschlag => "FLEXIBILITAET_ZUSCHLAG",
        }
    }

    /// Parse a storage token; `None` for anything outside the vocabulary.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.token() == token)
    }
}

/// One head of the EnFG Belastungsausgleich a paid amount is reported under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EnfgPosten {
    /// A payment for Strom nach § 19 Abs. 1 EEG — Marktprämie, Einspeisevergütung
    /// or Mieterstromzuschlag. § 13 Abs. 1 Satz 1 Nr. 1 (Nr. 3 for an earlier
    /// Fassung) EnFG; reported under § 50 Nr. 1 Buchst. a aa EnFG.
    Eeg19Abs1,
    /// A payment for „die Bereitstellung von installierter Leistung nach § 50"
    /// EEG — Flexibilitätszuschlag (§ 50a) or Flexibilitätsprämie (§ 50b).
    /// § 13 Abs. 1 Satz 1 Nr. 1 EnFG; reported under § 50 Nr. 1 Buchst. a bb.
    Eeg50,
    /// A payment „nach Maßgabe des Abschnitts 2 des Kraft-Wärme-Kopplungsgesetzes"
    /// — the § 7 KWKG Zuschlag sits in Abschnitt 2 (§§ 5–13b). § 13 Abs. 1
    /// Satz 1 Nr. 2 EnFG; reported under § 50 Nr. 1 Buchst. a cc.
    KwkgStrom,
    /// The Netzbetreiber's income from a § 52 EEG Pflichtzahlung — owed to the
    /// ÜNB under § 14 Satz 1 Nr. 3 EnFG; reported under § 50 Nr. 1 Buchst. e.
    Sect14Eeg52,
}

impl EnfgPosten {
    /// Every head, in reporting order.
    pub const ALL: [Self; 4] = [
        Self::Eeg19Abs1,
        Self::Eeg50,
        Self::KwkgStrom,
        Self::Sect14Eeg52,
    ];

    /// The storage token.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Eeg19Abs1 => "EEG_19_ABS1",
            Self::Eeg50 => "EEG_50",
            Self::KwkgStrom => "KWKG_STROM",
            Self::Sect14Eeg52 => "SECT14_EEG52",
        }
    }

    /// Parse a storage token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.token() == token)
    }

    /// The EnFG provision the head rests on.
    #[must_use]
    pub const fn rechtsgrundlage(self) -> &'static str {
        match self {
            Self::Eeg19Abs1 => "§ 13 Abs. 1 Satz 1 Nr. 1, 3 EnFG i.V.m. § 19 Abs. 1 EEG",
            Self::Eeg50 => "§ 13 Abs. 1 Satz 1 Nr. 1 EnFG i.V.m. § 50 EEG",
            Self::KwkgStrom => "§ 13 Abs. 1 Satz 1 Nr. 2, 3 EnFG i.V.m. Abschnitt 2 KWKG",
            Self::Sect14Eeg52 => "§ 14 Satz 1 Nr. 3 EnFG i.V.m. § 52 EEG",
        }
    }

    /// The item of the § 50 Nr. 1 EnFG Mitteilung the head is reported under.
    #[must_use]
    pub const fn mitteilungsposten(self) -> &'static str {
        match self {
            Self::Eeg19Abs1 => "§ 50 Nr. 1 Buchst. a Doppelbuchst. aa EnFG",
            Self::Eeg50 => "§ 50 Nr. 1 Buchst. a Doppelbuchst. bb EnFG",
            Self::KwkgStrom => "§ 50 Nr. 1 Buchst. a Doppelbuchst. cc EnFG",
            Self::Sect14Eeg52 => "§ 50 Nr. 1 Buchst. e EnFG",
        }
    }

    /// Whether the head is a § 13 Abs. 1 reimbursement owed **to** the
    /// Netzbetreiber; `false` for a § 14 item it owes the ÜNB.
    #[must_use]
    pub const fn ist_erstattung(self) -> bool {
        matches!(self, Self::Eeg19Abs1 | Self::Eeg50 | Self::KwkgStrom)
    }
}

/// Where a settlement model's payment lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnfgZuordnung {
    /// The model carries no Netzbetreiber payment, so nothing is reimbursed.
    Keine {
        /// Why, with its source.
        grund: &'static str,
    },
    /// The whole payment is one head.
    Ein(EnfgPosten),
    /// The payment is a § 19 Abs. 1 claim with a § 50 payment on top.
    ///
    /// § 50b Satz 1 EEG 2023 grants the Flexibilitätsprämie „ergänzend zu einer
    /// Veräußerung des Stroms", and § 50 Abs. 1 leaves the § 19 claim „unberührt",
    /// so one settlement pays two heads; the § 50 part is the settlement's
    /// Flexibilitätsprämie position and the rest is the § 19 part.
    Geteilt {
        /// The Strom payment — [`EnfgPosten::Eeg19Abs1`].
        grundanspruch: EnfgPosten,
        /// The capacity payment — [`EnfgPosten::Eeg50`].
        flexibilitaet: EnfgPosten,
    },
}

/// A model whose payment basis is not cited, so its payment has no head.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("settlement model {model}: no EnFG head — {grund}")]
pub struct KeinEnfgPosten {
    /// The model's storage token.
    pub model: &'static str,
    /// What is missing before it can be placed.
    pub grund: &'static str,
}

/// The EnFG head of `model`'s payment.
///
/// The `match` is exhaustive: a thirteenth model is a compile error here, not
/// a payment that silently lands under a default head.
///
/// # Errors
///
/// [`KeinEnfgPosten`] for `POST_EEG_SPOT` and `GGV`, whose payment basis is not
/// cited — reporting their payments under § 19 Abs. 1 would claim a
/// reimbursement the settlement may not rest on.
pub fn zuordnung(model: SettlementModel) -> Result<EnfgZuordnung, KeinEnfgPosten> {
    use EnfgPosten::{Eeg19Abs1, Eeg50, KwkgStrom};
    use SettlementModel as M;
    match model {
        // § 19 Abs. 1 Nr. 1 (Marktprämie nach § 20), Nr. 2 (Einspeisevergütung
        // nach § 21 Abs. 1 Nr. 1–4, the Ausfallvergütung being Nr. 3) and Nr. 3
        // (Mieterstromzuschlag nach § 21 Abs. 3) EEG 2023.
        M::Verguetung
        | M::Ausfallverguetung
        | M::Direktvermarktung
        | M::Ausschreibung
        | M::Mieterstrom => Ok(EnfgZuordnung::Ein(Eeg19Abs1)),
        M::Flexibilitaet => Ok(EnfgZuordnung::Geteilt {
            grundanspruch: Eeg19Abs1,
            flexibilitaet: Eeg50,
        }),
        // § 50a EEG 2023 — the Flexibilitätszuschlag is the § 50 claim alone.
        M::FlexibilitaetZuschlag => Ok(EnfgZuordnung::Ein(Eeg50)),
        M::KwkgZuschlag => Ok(EnfgZuordnung::Ein(KwkgStrom)),
        M::Eigenverbrauch => Ok(EnfgZuordnung::Keine {
            grund: "no feed-in, so no § 19 Abs. 1 EEG 2023 claim against the Netzbetreiber",
        }),
        M::SonstigeDirektvermarktung => Ok(EnfgZuordnung::Keine {
            grund: "§ 21b Abs. 1 Nr. 4 EEG 2023 sonstige Direktvermarktung nach § 21a: the \
                    plant is paid on the market, not by the Netzbetreiber",
        }),
        // § 23b EEG 2023 prices an ausgeförderte Anlage's Einspeisevergütung
        // (§ 19 Abs. 1 Nr. 2 i.V.m. § 21 Abs. 1 Satz 1 Nr. 4) at the Jahresmarktwert,
        // capped at 10 ct/kWh, and § 53 Abs. 4 deducts the ÜNB-published value
        // from it. The model settles on the monthly EPEX average without that
        // deduction, so whether its payment is the § 19 claim is not established.
        M::PostEegSpot => Err(KeinEnfgPosten {
            model: model.token(),
            grund: "the payment is not reconciled with § 23b and § 53 Abs. 4 EEG 2023 \
                    (Jahresmarktwert less the § 51 Abs. 1 Nr. 4 Buchst. b EnFG value), so it \
                    is not shown to be the § 19 Abs. 1 Nr. 2 claim",
        }),
        // § 21b Abs. 1 EEG 2023 lists four Veräußerungsformen and the GGV
        // (§ 42b EnWG) is none of them, so the token does not say which § 19
        // Abs. 1 claim, if any, the settlement pays.
        M::Ggv => Err(KeinEnfgPosten {
            model: model.token(),
            grund: "the gemeinschaftliche Gebäudeversorgung (§ 42b EnWG) is no § 21b Abs. 1 \
                    EEG 2023 Veräußerungsform, so which § 19 Abs. 1 claim the settlement pays \
                    is not cited",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_round_trips() {
        for m in SettlementModel::ALL {
            assert_eq!(SettlementModel::from_token(m.token()), Some(m));
        }
        assert_eq!(SettlementModel::from_token("SOLAR"), None);
        for p in EnfgPosten::ALL {
            assert_eq!(EnfgPosten::from_token(p.token()), Some(p));
        }
    }

    /// § 19 Abs. 1 Nr. 1–3 EEG 2023: Marktprämie, Einspeisevergütung (§ 21
    /// Abs. 1 Nr. 1–4, the Ausfallvergütung among them) and Mieterstromzuschlag
    /// are all one § 50 Nr. 1 Buchst. a aa item.
    #[test]
    fn the_sect19_models_land_under_aa() {
        for m in [
            SettlementModel::Verguetung,
            SettlementModel::Ausfallverguetung,
            SettlementModel::Direktvermarktung,
            SettlementModel::Ausschreibung,
            SettlementModel::Mieterstrom,
        ] {
            assert_eq!(
                zuordnung(m),
                Ok(EnfgZuordnung::Ein(EnfgPosten::Eeg19Abs1)),
                "{}",
                m.token()
            );
        }
    }

    #[test]
    fn the_flexibilitaetspraemie_splits_into_a_sect19_and_a_sect50_part() {
        assert_eq!(
            zuordnung(SettlementModel::Flexibilitaet),
            Ok(EnfgZuordnung::Geteilt {
                grundanspruch: EnfgPosten::Eeg19Abs1,
                flexibilitaet: EnfgPosten::Eeg50,
            })
        );
        assert_eq!(
            zuordnung(SettlementModel::FlexibilitaetZuschlag),
            Ok(EnfgZuordnung::Ein(EnfgPosten::Eeg50))
        );
    }

    #[test]
    fn the_kwk_zuschlag_is_the_kwkg_head() {
        assert_eq!(
            zuordnung(SettlementModel::KwkgZuschlag),
            Ok(EnfgZuordnung::Ein(EnfgPosten::KwkgStrom))
        );
    }

    #[test]
    fn a_model_without_a_payment_maps_to_none() {
        for m in [
            SettlementModel::Eigenverbrauch,
            SettlementModel::SonstigeDirektvermarktung,
        ] {
            assert!(
                matches!(zuordnung(m), Ok(EnfgZuordnung::Keine { .. })),
                "{}",
                m.token()
            );
        }
    }

    #[test]
    fn post_eeg_spot_and_ggv_refuse_until_their_basis_is_cited() {
        for m in [SettlementModel::PostEegSpot, SettlementModel::Ggv] {
            let err = zuordnung(m).expect_err(m.token());
            assert_eq!(err.model, m.token());
        }
    }

    /// Every model has an answer — a head, „none", or a refusal naming itself.
    #[test]
    fn every_model_is_answered() {
        for m in SettlementModel::ALL {
            match zuordnung(m) {
                Ok(_) => {}
                Err(e) => assert_eq!(e.model, m.token()),
            }
        }
    }

    /// The § 14 head is owed to the ÜNB, never reimbursed.
    #[test]
    fn only_the_sect13_heads_are_reimbursements() {
        assert!(EnfgPosten::Eeg19Abs1.ist_erstattung());
        assert!(EnfgPosten::Eeg50.ist_erstattung());
        assert!(EnfgPosten::KwkgStrom.ist_erstattung());
        assert!(!EnfgPosten::Sect14Eeg52.ist_erstattung());
    }
}
