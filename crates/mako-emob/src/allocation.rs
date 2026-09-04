//! **The conservation invariant** — Anlage 6 §IV.1, executable.
//!
//! ```text
//! NGZ(t, richtung) = Σ zugeordnete Marktlokationen + Deltamenge
//! ```
//!
//! exactly, for every quarter hour, every direction and every version. The
//! Netzgangzeitreihe is what the VNB measured at the Übergabestelle; the parts
//! are what the LPB claims for each supplier's Bilanzkreis; the Deltamenge is
//! the remainder, which Anlage 6 §IV.2 books to a Bilanzkreis the LPB names, at
//! its own cost.
//!
//! # The Deltamenge is a quantity, not a rounding error
//!
//! It has a Bilanzkreis, it settles in money, and it is the LPB's exposure —
//! an unmetered draw, a session whose CDR has not arrived, the six-decimal cut
//! of a proportional split all land in it. Hence a field on
//! [`QuarterHourAllocation`] and a returned [`ConservationProof`].
//!
//! # Two shapes of the same call
//!
//! | Case | What happens | Delta |
//! |---|---|---|
//! | claims **under** the NGZ | every claim is met in full | `NGZ − Σ claims` |
//! | claims **over** the NGZ | every claim is cut back in proportion | zero up to the six-decimal cut, [`Ueberdeckung`] recorded |
//!
//! Both come out of one `metering::allocation::allocate` with each part capped
//! at its own claim. Over-claim is real — generation behind the Netzanschluss
//! feeds the charge points — and neither Anlage 6 nor the AWH resolves it;
//! proportional cut-back is the default, recorded on the row because routine
//! over-claim is a metering fault, not a rounding one.
//!
//! # Directions never net
//!
//! Bezug and Einspeisung settle as separate series: netting them inside a
//! quarter hour would let a V2G discharge cancel a neighbour's draw, and both
//! would leave their suppliers' Bilanzkreise. [`Richtung`] is part of the key
//! and each direction carries its own non-negative pool.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use metering::allocation::{AllocationBasis, AllocationPart, allocate};

use crate::error::EmobError;
use crate::ids::VirtualMaloId;
use crate::session::Viertelstunde;

pub use mako_mabis::Datenstatus;

/// Which way the energy flowed across the Übergabestelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Richtung {
    /// Energy drawn from the VNB's grid — charging.
    Bezug,
    /// Energy fed back into it — V2G discharge, or local generation exported.
    ///
    /// No published Zeitreihentyp covers an Einspeisungs-BK-SZR eMob, so a
    /// deployment holds these rows until its BIKO names one. Modelling the
    /// direction is nevertheless not optional: without it the two flows net.
    Einspeisung,
}

/// What a virtual Marktlokation is for.
///
/// The last two exist so that energy nobody recognised still reaches a real
/// supplier's Bilanzkreis instead of the Deltamenge. Anlage 6 §IV.1 obliges the
/// LPB to assign the whole BG; letting station losses and unknown tokens fall
/// through to the Delta-BK would satisfy the arithmetic while defeating the
/// obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MaloKind {
    /// One vehicle or one driver's contract.
    Vehicle,
    /// A device that is not a vehicle — a stationary battery, a heat pump
    /// behind the same Übergabestelle.
    Device,
    /// A household or Kundenanlage under the BK6-24-267 access path.
    Household,
    /// The station's own consumption: standby, lighting, cooling, cable losses.
    ///
    /// A real Marktlokation with a real supplier — usually the operator's own.
    Betriebsstrom,
    /// Energy drawn on a token no registry recognised.
    ///
    /// Also a real Marktlokation with a real supplier: the „Residualstrom"
    /// contract every operating model in the market carries. Not the Delta.
    Residual,
}

/// One claim on a quarter hour: this virtual Marktlokation drew this much.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anspruch {
    /// Whose claim.
    pub malo: VirtualMaloId,
    /// What it is for.
    pub kind: MaloKind,
    /// Claimed energy in kWh. Must not be negative.
    pub kwh: Decimal,
}

/// One virtual Marktlokation's settled share of a quarter hour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zuordnung {
    /// Whose share.
    pub malo: VirtualMaloId,
    /// What it is for.
    pub kind: MaloKind,
    /// What was claimed.
    pub anspruch_kwh: Decimal,
    /// What was actually assigned — at most the claim.
    pub kwh: Decimal,
}

impl Zuordnung {
    /// `true` when the claim was cut back because the quarter hour was
    /// over-claimed.
    #[must_use]
    pub fn gekuerzt(&self) -> bool {
        self.kwh < self.anspruch_kwh
    }
}

/// Recorded when the claims exceeded what the Netzgangzeitreihe delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ueberdeckung {
    /// What the claims added up to.
    pub anspruch_kwh: Decimal,
    /// What the NGZ delivered.
    pub ngz_kwh: Decimal,
}

impl Ueberdeckung {
    /// How much more was claimed than arrived.
    #[must_use]
    pub fn ueberhang_kwh(&self) -> Decimal {
        self.anspruch_kwh - self.ngz_kwh
    }
}

/// Proof that Anlage 6 §IV.1 holds for one quarter hour.
///
/// Returned rather than asserted, so a caller can file it beside the allocation
/// and an auditor can re-check it without re-running the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConservationProof {
    /// The measured Netzgangzeitreihe value.
    pub ngz_kwh: Decimal,
    /// What the parts add up to.
    pub zugeordnet_kwh: Decimal,
    /// The Deltamenge.
    pub delta_kwh: Decimal,
}

impl ConservationProof {
    /// `true` when `zugeordnet + delta == ngz` exactly.
    #[must_use]
    pub fn haelt(&self) -> bool {
        self.zugeordnet_kwh + self.delta_kwh == self.ngz_kwh
    }
}

/// One quarter hour of one Übergabestelle, allocated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarterHourAllocation {
    /// The quarter hour.
    pub slot: Viertelstunde,
    /// Which direction this row settles.
    pub richtung: Richtung,
    /// The Netzgangzeitreihe value the VNB measured.
    pub ngz_kwh: Decimal,
    /// Every virtual Marktlokation's share, in input order.
    pub zuordnungen: Vec<Zuordnung>,
    /// The Deltamenge — what no Marktlokation claimed.
    pub delta_kwh: Decimal,
    /// Present only when the quarter hour was over-claimed.
    pub ueberdeckung: Option<Ueberdeckung>,
    /// The conservation identity, ready to file.
    pub proof: ConservationProof,
}

impl QuarterHourAllocation {
    /// Allocate one quarter hour.
    ///
    /// # Errors
    ///
    /// [`EmobError::Allocation`] when `ngz_kwh` or any claim is negative — a
    /// reverse flow is [`Richtung::Einspeisung`], not a negative Bezug;
    /// [`EmobError::DoppelterAnspruch`] when one virtual Marktlokation claims
    /// the same quarter hour twice; and [`EmobError::ErhaltungVerletzt`] if the
    /// identity somehow fails, which is a bug in this crate rather than a
    /// caller condition.
    pub fn allocate(
        slot: Viertelstunde,
        richtung: Richtung,
        ngz_kwh: Decimal,
        ansprueche: &[Anspruch],
    ) -> Result<Self, EmobError> {
        if ngz_kwh < Decimal::ZERO {
            return Err(EmobError::Allocation(format!(
                "the Netzgangzeitreihe value {ngz_kwh} is negative; settle a reverse flow as \
                 Richtung::Einspeisung"
            )));
        }
        if let Some(bad) = ansprueche.iter().find(|a| a.kwh < Decimal::ZERO) {
            return Err(EmobError::Allocation(format!(
                "virtual Marktlokation {} claims negative energy {}",
                bad.malo, bad.kwh
            )));
        }
        // One row per Marktlokation, because one Marktlokation is one
        // Bilanzkreis-Zuordnung. Summing two claims silently would be the
        // friendlier default and the wrong one: the reason a MaLo appears
        // twice (two sessions, or the same CDR ingested twice) is knowable
        // upstream and not here.
        let mut gesehen = std::collections::BTreeSet::new();
        if let Some(dup) = ansprueche.iter().find(|a| !gesehen.insert(&a.malo)) {
            return Err(EmobError::DoppelterAnspruch {
                malo: dup.malo.to_string(),
            });
        }

        let anspruch_sum: Decimal = ansprueche.iter().map(|a| a.kwh).sum();

        let parts: Vec<AllocationPart> = ansprueche
            .iter()
            .map(|a| AllocationPart::new(a.malo.as_str(), a.kwh).capped_at(a.kwh))
            .collect();

        let row = allocate(ngz_kwh, parts, AllocationBasis::Proportional)?;

        let zuordnungen: Vec<Zuordnung> = ansprueche
            .iter()
            .zip(row.parts.iter())
            .map(|(a, p)| Zuordnung {
                malo: a.malo.clone(),
                kind: a.kind,
                anspruch_kwh: a.kwh,
                kwh: p.allocated,
            })
            .collect();

        let zugeordnet_kwh: Decimal = zuordnungen.iter().map(|z| z.kwh).sum();
        let delta_kwh = row.residual;

        let proof = ConservationProof {
            ngz_kwh,
            zugeordnet_kwh,
            delta_kwh,
        };
        if !proof.haelt() {
            return Err(EmobError::ErhaltungVerletzt {
                slot: slot.start().to_string(),
                ngz: ngz_kwh,
                summe: zugeordnet_kwh,
                delta: delta_kwh,
            });
        }

        let ueberdeckung = (anspruch_sum > ngz_kwh).then_some(Ueberdeckung {
            anspruch_kwh: anspruch_sum,
            ngz_kwh,
        });

        Ok(Self {
            slot,
            richtung,
            ngz_kwh,
            zuordnungen,
            delta_kwh,
            ueberdeckung,
            proof,
        })
    }

    /// The share that reached a given kind of Marktlokation.
    #[must_use]
    pub fn kwh_of_kind(&self, kind: MaloKind) -> Decimal {
        self.zuordnungen
            .iter()
            .filter(|z| z.kind == kind)
            .map(|z| z.kwh)
            .sum()
    }
}

/// One filing of an allocation, versioned the way MaBiS versions everything.
///
/// MaBiS Kap. 3.8.2 keys versions on the **Erstellungszeitpunkt** — a 17-char
/// timestamp, never an integer — and pairs it with a [`Datenstatus`]. Both are
/// reused from [`mako_mabis`] rather than restated, so a Modell-2 filing and an
/// ordinary Summenzeitreihe filing cannot disagree about what „final" means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocationVersion {
    /// When this version was formed.
    #[serde(with = "time::serde::rfc3339")]
    pub erstellungszeitpunkt: OffsetDateTime,
    /// Its MaBiS Datenstatus.
    pub datenstatus: Datenstatus,
    /// The quarter hours it covers.
    pub rows: Vec<QuarterHourAllocation>,
}

impl AllocationVersion {
    /// The total Deltamenge across every quarter hour and direction.
    ///
    /// The LPB's exposure for this version, in kWh.
    #[must_use]
    pub fn delta_kwh(&self) -> Decimal {
        self.rows.iter().map(|r| r.delta_kwh).sum()
    }

    /// Every quarter hour whose claims exceeded the Netzgangzeitreihe.
    pub fn ueberdeckungen(&self) -> impl Iterator<Item = &QuarterHourAllocation> {
        self.rows.iter().filter(|r| r.ueberdeckung.is_some())
    }

    /// `true` when the conservation identity holds for every row.
    #[must_use]
    pub fn erhaltung_haelt(&self) -> bool {
        self.rows.iter().all(|r| r.proof.haelt())
    }

    /// `true` when the Bilanzierungsmonat this version belongs to has settled.
    ///
    /// „Abgerechnete Daten" and „Abgerechnete Daten KBKA" have reached their
    /// Abrechnungsstichtag. Delegated to [`Datenstatus::ist_abgerechnet`]
    /// rather than restated: a second copy of the code list is a copy that can
    /// drift from the one `mako-mabis` publishes.
    #[must_use]
    pub fn ist_final(&self) -> bool {
        self.datenstatus.ist_abgerechnet()
    }
}

/// Every version filed for one Bilanzierungsmonat, in filing order.
///
/// The invariants below are properties of the *sequence*, which no single
/// [`AllocationVersion`] can state about itself — the same reason
/// [`crate::bg::BgRegistry`] exists beside
/// [`crate::bg::VirtualBalancingArea`].
///
/// | Invariant | Source |
/// |---|---|
/// | a version is immutable once filed; a correction is a **new** version | MaBiS Kap. 3.8.2 |
/// | the Erstellungszeitpunkt strictly increases | MaBiS Kap. 3.8.2 — versions are *keyed* on it, so two filings sharing one are indistinguishable |
/// | nothing follows an „abgerechnet" version | MaBiS Kap. 3.10 Tabelle 2 |
/// | nothing is filed after the end of month M+7 | MaBiS Kap. 3.10 |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versionsreihe {
    /// The last calendar day of the Bilanzierungsmonat. Stored as the date
    /// rather than as a [`mako_mabis::Bilanzierungsmonat`] because a series is
    /// persisted and that type is a computed view over exactly this value.
    monatsende: time::Date,
    versionen: Vec<AllocationVersion>,
}

impl Versionsreihe {
    /// An empty series for the Bilanzierungsmonat containing `tag`.
    #[must_use]
    pub fn fuer(tag: time::Date) -> Self {
        Self {
            monatsende: mako_mabis::Bilanzierungsmonat::enthaltend(tag).monatsende(),
            versionen: Vec::new(),
        }
    }

    /// The Bilanzierungsmonat this series settles.
    #[must_use]
    pub fn monat(&self) -> mako_mabis::Bilanzierungsmonat {
        mako_mabis::Bilanzierungsmonat::new(self.monatsende)
    }

    /// The last day a correction may be filed — the end of month M+7
    /// (MaBiS Kap. 3.10).
    #[must_use]
    pub fn korrekturfrist(&self) -> time::Date {
        crate::fristen::korrekturfrist(self.monat())
    }

    /// Every version filed so far, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &AllocationVersion> {
        self.versionen.iter()
    }

    /// The version that would settle today — the most recently filed one.
    #[must_use]
    pub fn aktuell(&self) -> Option<&AllocationVersion> {
        self.versionen.last()
    }

    /// File `version`, as of `eingang`.
    ///
    /// # Errors
    ///
    /// - [`EmobError::VersionIstFinal`] when the series has already settled.
    /// - [`EmobError::KorrekturfristAbgelaufen`] when `eingang` is past the
    ///   end of month M+7.
    /// - [`EmobError::Allocation`] when the Erstellungszeitpunkt does not
    ///   advance, or when the version's conservation identity does not hold —
    ///   a version that fails Anlage 6 §IV.1 is not a version to file.
    pub fn einreichen(
        &mut self,
        version: AllocationVersion,
        eingang: time::Date,
    ) -> Result<(), EmobError> {
        if let Some(letzte) = self.versionen.last() {
            if letzte.ist_final() {
                return Err(EmobError::VersionIstFinal {
                    erstellungszeitpunkt: letzte.erstellungszeitpunkt.to_string(),
                });
            }
            if version.erstellungszeitpunkt <= letzte.erstellungszeitpunkt {
                return Err(EmobError::Allocation(format!(
                    "Erstellungszeitpunkt {} does not advance on the filed {}; MaBiS keys \
                     versions on it",
                    version.erstellungszeitpunkt, letzte.erstellungszeitpunkt
                )));
            }
        }
        let frist = self.korrekturfrist();
        if eingang > frist {
            return Err(EmobError::KorrekturfristAbgelaufen {
                monat: format!(
                    "{}-{:02}",
                    self.monatsende.year(),
                    u8::from(self.monatsende.month())
                ),
                frist,
                eingang,
            });
        }
        if !version.erhaltung_haelt() {
            return Err(EmobError::Allocation(
                "a version whose conservation identity fails cannot be filed (Anlage 6 §IV.1)"
                    .to_owned(),
            ));
        }
        self.versionen.push(version);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn slot() -> Viertelstunde {
        Viertelstunde::containing(datetime!(2026-11-03 08:00:00 UTC))
    }

    fn a(id: &str, kind: MaloKind, kwh: Decimal) -> Anspruch {
        Anspruch {
            malo: VirtualMaloId::new(id).unwrap(),
            kind,
            kwh,
        }
    }

    #[test]
    fn an_underclaimed_quarter_hour_puts_the_rest_in_the_delta() {
        let r = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(12),
            &[
                a("veh-1", MaloKind::Vehicle, dec!(6)),
                a("veh-2", MaloKind::Vehicle, dec!(3)),
            ],
        )
        .unwrap();
        assert_eq!(r.zuordnungen[0].kwh, dec!(6));
        assert_eq!(r.zuordnungen[1].kwh, dec!(3));
        assert_eq!(r.delta_kwh, dec!(3));
        assert!(r.ueberdeckung.is_none());
        assert!(r.proof.haelt());
    }

    #[test]
    fn an_overclaimed_quarter_hour_cuts_back_proportionally_and_says_so() {
        let r = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(10),
            &[
                a("veh-1", MaloKind::Vehicle, dec!(10)),
                a("veh-2", MaloKind::Vehicle, dec!(10)),
            ],
        )
        .unwrap();
        assert_eq!(r.zuordnungen[0].kwh, dec!(5));
        assert_eq!(r.zuordnungen[1].kwh, dec!(5));
        assert_eq!(r.delta_kwh, Decimal::ZERO);
        let u = r.ueberdeckung.expect("recorded, never silent");
        assert_eq!(u.ueberhang_kwh(), dec!(10));
        assert!(r.zuordnungen.iter().all(Zuordnung::gekuerzt));
        assert!(r.proof.haelt());
    }

    /// Station losses and unknown tokens reach a supplier, not the Delta.
    #[test]
    fn betriebsstrom_and_residual_are_ordinary_claims() {
        let r = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(10),
            &[
                a("veh-1", MaloKind::Vehicle, dec!(7)),
                a("station-1", MaloKind::Betriebsstrom, dec!(1)),
                a("residual", MaloKind::Residual, dec!(2)),
            ],
        )
        .unwrap();
        assert_eq!(r.delta_kwh, Decimal::ZERO);
        assert_eq!(r.kwh_of_kind(MaloKind::Betriebsstrom), dec!(1));
        assert_eq!(r.kwh_of_kind(MaloKind::Residual), dec!(2));
        assert_eq!(r.kwh_of_kind(MaloKind::Vehicle), dec!(7));
    }

    /// The whole quarter hour becomes Delta when nobody claims it.
    #[test]
    fn no_claims_means_the_whole_slot_is_delta() {
        let r = QuarterHourAllocation::allocate(slot(), Richtung::Bezug, dec!(4), &[]).unwrap();
        assert_eq!(r.delta_kwh, dec!(4));
        assert!(r.proof.haelt());
    }

    #[test]
    fn a_zero_ngz_allocates_nothing() {
        let r = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            Decimal::ZERO,
            &[a("veh-1", MaloKind::Vehicle, dec!(5))],
        )
        .unwrap();
        assert_eq!(r.zuordnungen[0].kwh, Decimal::ZERO);
        assert_eq!(r.delta_kwh, Decimal::ZERO);
        assert!(r.proof.haelt());
    }

    /// Thirds do not divide, and the identity still has to hold exactly.
    #[test]
    fn conservation_survives_a_non_terminating_share() {
        let r = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(10),
            &[
                a("v1", MaloKind::Vehicle, dec!(10)),
                a("v2", MaloKind::Vehicle, dec!(10)),
                a("v3", MaloKind::Vehicle, dec!(10)),
            ],
        )
        .unwrap();
        assert!(r.proof.haelt());
        let sum: Decimal = r.zuordnungen.iter().map(|z| z.kwh).sum();
        assert_eq!(sum + r.delta_kwh, dec!(10));
    }

    /// One Marktlokation is one Bilanzkreis-Zuordnung, so two claims for it
    /// are a caller bug rather than a sum.
    #[test]
    fn a_marktlokation_may_not_claim_the_same_slot_twice() {
        let e = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(10),
            &[
                a("veh-1", MaloKind::Vehicle, dec!(4)),
                a("veh-1", MaloKind::Vehicle, dec!(3)),
            ],
        )
        .unwrap_err();
        assert!(matches!(e, EmobError::DoppelterAnspruch { .. }), "{e:?}");
    }

    #[test]
    fn negative_inputs_are_refused() {
        assert!(QuarterHourAllocation::allocate(slot(), Richtung::Bezug, dec!(-1), &[]).is_err());
        assert!(
            QuarterHourAllocation::allocate(
                slot(),
                Richtung::Bezug,
                dec!(1),
                &[a("v1", MaloKind::Vehicle, dec!(-1))]
            )
            .is_err()
        );
    }

    /// Directions are separate pools and never net against each other.
    #[test]
    fn the_two_directions_are_settled_apart() {
        let bezug = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(10),
            &[a("v1", MaloKind::Vehicle, dec!(10))],
        )
        .unwrap();
        let einspeisung = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Einspeisung,
            dec!(4),
            &[a("v1", MaloKind::Vehicle, dec!(4))],
        )
        .unwrap();
        assert_eq!(bezug.zuordnungen[0].kwh, dec!(10));
        assert_eq!(einspeisung.zuordnungen[0].kwh, dec!(4));
        assert_ne!(bezug.richtung, einspeisung.richtung);
    }

    #[test]
    fn a_version_totals_its_delta_and_flags_its_overclaims() {
        let good = QuarterHourAllocation::allocate(
            slot(),
            Richtung::Bezug,
            dec!(12),
            &[a("v1", MaloKind::Vehicle, dec!(9))],
        )
        .unwrap();
        let over = QuarterHourAllocation::allocate(
            slot().next(),
            Richtung::Bezug,
            dec!(5),
            &[a("v1", MaloKind::Vehicle, dec!(8))],
        )
        .unwrap();
        let v = AllocationVersion {
            erstellungszeitpunkt: datetime!(2026-11-04 09:00:00 UTC),
            datenstatus: Datenstatus::Pruefdaten,
            rows: vec![good, over],
        };
        assert_eq!(v.delta_kwh(), dec!(3));
        assert_eq!(v.ueberdeckungen().count(), 1);
        assert!(v.erhaltung_haelt());
        assert!(!v.ist_final());
    }

    fn version(stamp: OffsetDateTime, status: Datenstatus) -> AllocationVersion {
        AllocationVersion {
            erstellungszeitpunkt: stamp,
            datenstatus: status,
            rows: vec![
                QuarterHourAllocation::allocate(
                    slot(),
                    Richtung::Bezug,
                    dec!(10),
                    &[a("v1", MaloKind::Vehicle, dec!(6))],
                )
                .unwrap(),
            ],
        }
    }

    fn tag(y: i32, m: u8, d: u8) -> time::Date {
        time::Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), d).unwrap()
    }

    #[test]
    fn a_series_takes_corrections_until_the_month_settles() {
        let mut reihe = Versionsreihe::fuer(tag(2026, 11, 15));
        assert_eq!(reihe.korrekturfrist(), tag(2027, 6, 30));

        reihe
            .einreichen(
                version(datetime!(2026-12-05 09:00:00 UTC), Datenstatus::Pruefdaten),
                tag(2026, 12, 5),
            )
            .unwrap();
        reihe
            .einreichen(
                version(
                    datetime!(2027-01-08 09:00:00 UTC),
                    Datenstatus::Abrechnungsdaten,
                ),
                tag(2027, 1, 8),
            )
            .unwrap();
        assert_eq!(reihe.iter().count(), 2);
        assert_eq!(
            reihe.aktuell().unwrap().datenstatus,
            Datenstatus::Abrechnungsdaten
        );
    }

    /// „Abgerechnet" closes the series; a later correction is not a filing.
    #[test]
    fn nothing_follows_an_abgerechnete_version() {
        let mut reihe = Versionsreihe::fuer(tag(2026, 11, 15));
        reihe
            .einreichen(
                version(
                    datetime!(2027-01-08 09:00:00 UTC),
                    Datenstatus::AbgerechneteDaten,
                ),
                tag(2027, 1, 8),
            )
            .unwrap();
        let e = reihe
            .einreichen(
                version(datetime!(2027-02-08 09:00:00 UTC), Datenstatus::Pruefdaten),
                tag(2027, 2, 8),
            )
            .unwrap_err();
        assert!(matches!(e, EmobError::VersionIstFinal { .. }), "{e:?}");
    }

    #[test]
    fn a_filing_past_month_seven_is_refused() {
        let mut reihe = Versionsreihe::fuer(tag(2026, 11, 15));
        let e = reihe
            .einreichen(
                version(datetime!(2027-07-01 09:00:00 UTC), Datenstatus::Pruefdaten),
                tag(2027, 7, 1),
            )
            .unwrap_err();
        match e {
            EmobError::KorrekturfristAbgelaufen { monat, frist, .. } => {
                assert_eq!(monat, "2026-11");
                assert_eq!(frist, tag(2027, 6, 30));
            }
            other => panic!("{other:?}"),
        }
    }

    /// MaBiS keys versions on the Erstellungszeitpunkt, so two filings may not
    /// share one.
    #[test]
    fn the_erstellungszeitpunkt_has_to_advance() {
        let mut reihe = Versionsreihe::fuer(tag(2026, 11, 15));
        let stamp = datetime!(2026-12-05 09:00:00 UTC);
        reihe
            .einreichen(version(stamp, Datenstatus::Pruefdaten), tag(2026, 12, 5))
            .unwrap();
        assert!(
            reihe
                .einreichen(version(stamp, Datenstatus::Pruefdaten), tag(2026, 12, 5))
                .is_err()
        );
    }

    #[test]
    fn settled_versions_are_final() {
        for status in [
            Datenstatus::AbgerechneteDaten,
            Datenstatus::AbgerechneteDatenKbka,
        ] {
            let v = AllocationVersion {
                erstellungszeitpunkt: datetime!(2026-11-04 09:00:00 UTC),
                datenstatus: status,
                rows: Vec::new(),
            };
            assert!(v.ist_final(), "{status:?}");
        }
        for status in [
            Datenstatus::Pruefdaten,
            Datenstatus::Abrechnungsdaten,
            Datenstatus::AbrechnungsdatenKbka,
        ] {
            let v = AllocationVersion {
                erstellungszeitpunkt: datetime!(2026-11-04 09:00:00 UTC),
                datenstatus: status,
                rows: Vec::new(),
            };
            assert!(!v.ist_final(), "{status:?}");
        }
    }
}
