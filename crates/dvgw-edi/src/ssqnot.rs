//! SSQNOT — the Mehr-/Mindermengenmeldung zur Führung des Netzkontos, read
//! as one business record.
//!
//! SSQNOT 5.7 (ORDRSP / UN D.07A S3) carries, per `LIN` position, one
//! `LOC+Z99` group with the Abrechnungszeitraum (`DTM+2`, format `719`), one
//! `QTY` — `ZY0` Mehrmenge or `ZY2` Mindermenge, a natural number in `KWH` —
//! its Verfahren in `STS` (`A1G` SLP, `A2G` RLM) and the Netzkontonummer in
//! `SG39 NAD+ZSH`. The DVGW column admits two positions, so a message states
//! the Mehr- and the Mindermenge of one Netzkonto and period.
//!
//! The receiver assigns it by the 2-Tupel (Netzkonto, Netzbetreiber) — `SG39
//! NAD+ZSH` and `SG3 NAD+MS` (§3.3), [`Zuordnung::MehrMindermengen`].
//!
//! [`Zuordnung::MehrMindermengen`]: crate::Zuordnung::MehrMindermengen

use rust_decimal::Decimal;

use crate::{
    datetime::DvgwPeriod,
    document::DvgwMessageType,
    message::DvgwMessage,
    model::{nad, qty, sts},
};

/// How the Mehr-/Mindermenge was determined — `SG37 STS` DE 9015.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Verfahren {
    /// `A1G` — Standardlastprofil.
    Slp,
    /// `A2G` — registrierende Leistungsmessung; Zeiträume before 1.10.2015 only.
    Rlm,
}

impl Verfahren {
    /// Parse the `STS` DE 9015 code.
    #[must_use]
    pub fn from_sts_code(code: &str) -> Option<Self> {
        match code {
            sts::SLP => Some(Self::Slp),
            sts::RLM => Some(Self::Rlm),
            _ => None,
        }
    }

    /// The `STS` DE 9015 code.
    #[must_use]
    pub fn sts_code(self) -> &'static str {
        match self {
            Self::Slp => sts::SLP,
            Self::Rlm => sts::RLM,
        }
    }
}

/// Why a message could not be read as a Mehr-/Mindermengenmeldung.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SsqnotError {
    /// The message is another family.
    #[error("not a SSQNOT: the message is {0}")]
    NotSsqnot(DvgwMessageType),
    /// No `DTM+Z01` Abrechnungszeitraum could be read.
    #[error("no usable DTM+Z01 Abrechnungszeitraum")]
    NoPeriod,
    /// No position carries a `NAD+ZSH` Netzkontonummer.
    #[error("no SG39 NAD+ZSH Netzkontonummer")]
    NoNetzkonto,
    /// The positions name more than one Netzkonto.
    #[error("the positions name more than one Netzkonto")]
    SeveralNetzkonten,
    /// A quantity carries no `STS`, or the positions disagree on it.
    #[error("no single Verfahren: every QTY needs STS+A1G or STS+A2G, all the same")]
    NoVerfahren,
    /// A `QTY` is not a number.
    #[error("QTY+{0} is not numeric")]
    NotNumeric(String),
    /// A `QTY` qualifier is neither `ZY0` nor `ZY2`.
    #[error("QTY+{0} is neither ZY0 Mehrmenge nor ZY2 Mindermenge")]
    UnknownQualifier(String),
}

/// The substance of one SSQNOT.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MehrMindermengenmeldung {
    /// `SG39 NAD+ZSH` — the Netzkonto the quantities are booked against.
    pub netzkonto: String,
    /// `SG3 NAD+MS` — the reporting Netzbetreiber.
    pub netzbetreiber: String,
    /// `DTM+Z01` — the Abrechnungszeitraum.
    pub zeitraum: DvgwPeriod,
    /// `STS` — SLP or RLM.
    pub verfahren: Verfahren,
    /// `QTY+ZY0` in kWh; zero when the message states none.
    pub mehrmenge_kwh: Decimal,
    /// `QTY+ZY2` in kWh; zero when the message states none.
    pub mindermenge_kwh: Decimal,
}

impl MehrMindermengenmeldung {
    /// Read the record out of a parsed SSQNOT.
    ///
    /// # Errors
    ///
    /// See [`SsqnotError`]. A message that validates clean reads without error.
    pub fn from_message(msg: &DvgwMessage) -> Result<Self, SsqnotError> {
        if msg.message_type != DvgwMessageType::Ssqnot {
            return Err(SsqnotError::NotSsqnot(msg.message_type));
        }
        let zeitraum = msg.validity_period.ok_or(SsqnotError::NoPeriod)?;
        let netzbetreiber = msg.sender().map(|p| p.id.clone()).unwrap_or_default();

        let mut netzkonto: Option<String> = None;
        let mut verfahren: Option<Verfahren> = None;
        let mut mehrmenge = Decimal::ZERO;
        let mut mindermenge = Decimal::ZERO;
        for item in &msg.items {
            if let Some(party) = item.party(nad::NETZKONTO_ZO_T3) {
                match &netzkonto {
                    Some(known) if known != &party.id => {
                        return Err(SsqnotError::SeveralNetzkonten);
                    }
                    Some(_) => {}
                    None => netzkonto = Some(party.id.clone()),
                }
            }
            for quantity in item.quantities() {
                let this = quantity
                    .status_code()
                    .and_then(Verfahren::from_sts_code)
                    .ok_or(SsqnotError::NoVerfahren)?;
                if verfahren.is_some_and(|v| v != this) {
                    return Err(SsqnotError::NoVerfahren);
                }
                verfahren = Some(this);
                let value = quantity
                    .value
                    .ok_or_else(|| SsqnotError::NotNumeric(quantity.qualifier.clone()))?;
                match quantity.qualifier.as_str() {
                    qty::MEHRMENGE => mehrmenge += value,
                    qty::MINDERMENGE => mindermenge += value,
                    other => return Err(SsqnotError::UnknownQualifier(other.to_owned())),
                }
            }
        }
        Ok(Self {
            netzkonto: netzkonto.ok_or(SsqnotError::NoNetzkonto)?,
            netzbetreiber,
            zeitraum,
            verfahren: verfahren.ok_or(SsqnotError::NoVerfahren)?,
            mehrmenge_kwh: mehrmenge,
            mindermenge_kwh: mindermenge,
        })
    }

    /// Mehrmenge minus Mindermenge, in kWh — positive when the Netzkonto
    /// received more than was allocated to it.
    #[must_use]
    pub fn saldo_kwh(&self) -> Decimal {
        self.mehrmenge_kwh - self.mindermenge_kwh
    }
}
