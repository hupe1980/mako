//! The Widerruf of a consumer's online contract — the pure verdicts.
//!
//! The Frist itself is `mako_fristen::vorlauf::widerrufsfrist`, anchored on the
//! Vertragsschluss the contract carries. What lives here is the one decision
//! that reads the Bestellnachweis: whether energy delivered before the
//! Widerruf is owed for.

use time::Date;

/// The two § 357a Abs. 2 Satz 1 BGB facts the Bestellnachweis records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WertersatzNachweis {
    /// Nr. 1: „der Verbraucher von dem Unternehmer ausdrücklich verlangt hat,
    /// dass mit der Leistung vor Ablauf der Widerrufsfrist begonnen werden
    /// soll".
    pub lieferung_verlangt: bool,
    /// Nr. 3: „der Unternehmer den Verbraucher nach Artikel 246a § 1 Absatz 2
    /// Satz 1 Nummer 1 und 3 … ordnungsgemäß informiert hat" — the
    /// Widerrufsbelehrung and the Wertersatz information, both shown before
    /// the order.
    pub belehrung_angezeigt: bool,
    pub wertersatz_info_angezeigt: bool,
}

/// Whether the consumer owes Wertersatz for Strom or Gas delivered before the
/// Widerruf (§ 357a Abs. 2 Satz 1 BGB).
///
/// - `None` when supply had not begun by the Widerruf (`widerruf` before
///   `lieferbeginn`): nothing was delivered, so there is nothing to decide.
/// - `Some(true)` only when the record holds Nr. 1 **and** Nr. 3. Nr. 2 binds
///   off-premises contracts, which the order route does not take.
/// - `Some(false)` otherwise.
///
/// Whether the agreed price is „unverhältnismäßig hoch" (Satz 3) is not
/// decided here; the basis is „der vereinbarte Gesamtpreis" (Satz 2).
#[must_use]
pub fn wertersatz(
    nachweis: WertersatzNachweis,
    lieferbeginn: Date,
    widerruf: Date,
) -> Option<bool> {
    if widerruf < lieferbeginn {
        return None;
    }
    Some(
        nachweis.lieferung_verlangt
            && nachweis.belehrung_angezeigt
            && nachweis.wertersatz_info_angezeigt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    const VOLL: WertersatzNachweis = WertersatzNachweis {
        lieferung_verlangt: true,
        belehrung_angezeigt: true,
        wertersatz_info_angezeigt: true,
    };

    #[test]
    fn with_nr_1_and_nr_3_wertersatz_is_owed() {
        assert_eq!(
            wertersatz(VOLL, date!(2026 - 12 - 01), date!(2026 - 12 - 05)),
            Some(true)
        );
    }

    /// § 357a Abs. 2 Satz 1 Nr. 1 BGB.
    #[test]
    fn no_express_request_means_no_wertersatz() {
        let n = WertersatzNachweis {
            lieferung_verlangt: false,
            ..VOLL
        };
        assert_eq!(
            wertersatz(n, date!(2026 - 12 - 01), date!(2026 - 12 - 05)),
            Some(false)
        );
    }

    /// § 357a Abs. 2 Satz 1 Nr. 3 BGB — both Art. 246a § 1 Abs. 2 Satz 1
    /// Nr. 1 and Nr. 3 EGBGB informations.
    #[test]
    fn an_express_request_without_the_nr_3_information_means_no_wertersatz() {
        for n in [
            WertersatzNachweis {
                wertersatz_info_angezeigt: false,
                ..VOLL
            },
            WertersatzNachweis {
                belehrung_angezeigt: false,
                ..VOLL
            },
        ] {
            assert_eq!(
                wertersatz(n, date!(2026 - 12 - 01), date!(2026 - 12 - 05)),
                Some(false)
            );
        }
    }

    #[test]
    fn before_lieferbeginn_wertersatz_is_not_decided() {
        assert_eq!(
            wertersatz(VOLL, date!(2026 - 12 - 01), date!(2026 - 11 - 30)),
            None
        );
    }
}
