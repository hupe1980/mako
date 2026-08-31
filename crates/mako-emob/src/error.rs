//! What can go wrong forming a Modell-2 allocation.

use rust_decimal::Decimal;
use thiserror::Error;

/// An invariant of Anlage 6 / the AWH „Zum Modell 2" that the caller's data
/// breaks.
///
/// Every variant names the clause it enforces. None of them is a wire error:
/// this crate does no I/O, so all of these are statements about the operator's
/// own records.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EmobError {
    /// A Bilanzierungsgebiet may begin, change or end only on the first of a
    /// month (AWH Kap. 5.3.1.3: „Die Bildung und Änderung von einem BG erfolgt
    /// nur zum Ersten eines Monats").
    #[error("{was} must fall on the first of a month, got {date}")]
    NotFirstOfMonth {
        /// Which lifecycle instant was wrong.
        was: &'static str,
        /// The date the caller supplied.
        date: time::Date,
    },

    /// A Ladepunktbetreiber holds at most one Bilanzierungsgebiet per Regelzone
    /// (AWH Kap. 5.3).
    #[error("the LPB already holds a Bilanzierungsgebiet in Regelzone {regelzone}")]
    DoppeltesBilanzierungsgebiet {
        /// The Regelzone that already has one.
        regelzone: String,
    },

    /// The Übergabestelle's Anmeldedatum precedes the BG's Gültigkeitsbeginn
    /// (AWH Kap. 2.1.2 Nr. 1: „Das Anmeldedatum darf nicht vor dem
    /// Gültigkeitsbeginn des BG liegen").
    #[error("Anmeldedatum {anmeldung} precedes the Gültigkeitsbeginn {bg_start} of the BG")]
    AnmeldungVorBgBeginn {
        /// The requested Anmeldedatum.
        anmeldung: time::Date,
        /// The BG's Gültigkeitsbeginn.
        bg_start: time::Date,
    },

    /// The Anmeldedatum falls on or after the day the BG stops being valid.
    ///
    /// The AWH states only the lower bound, because a BG that has been
    /// beendet is not a BG one can register against at all: a Marktlokation
    /// balanced into an expired Bilanzierungsgebiet has no Bilanzkreis-
    /// Zuordnung on the day it takes effect, and the BIKO has nothing to
    /// settle it against (AWH Kap. 5.3.1.3, Anlage 6 §II).
    #[error("Anmeldedatum {anmeldung} is not before the Gültigkeitsende {bg_ende} of the BG")]
    AnmeldungNachBgEnde {
        /// The requested Anmeldedatum.
        anmeldung: time::Date,
        /// The first day the BG is no longer valid.
        bg_ende: time::Date,
    },

    /// A Kundenanlage carrying an EEG-geförderte Anlage was onboarded on the
    /// BK6-24-267 path.
    ///
    /// The Beschluss is explicit that its scope excludes „Kundenanlagen mit
    /// EEG-geförderten Anlagen oder anderweitig komplexen Strukturen"
    /// (S. 28). It does not decide the case either way, so the model cannot be
    /// applied to it without a bilateral agreement — and applying it anyway
    /// would balance an EEG-vergütete Einspeisung into the LPB's BG.
    #[error(
        "BK6-24-267 S. 28 excludes a Kundenanlage with an EEG-geförderte Anlage from \
         individueller Netzzugang; record a bilateral agreement before onboarding"
    )]
    EegAnlageAusserhalbDesBeschlusses,

    /// The Übergabestelle is not quarter-hour metered.
    ///
    /// Anlage 6 §III.1 makes ¼-h measurement — a Zählerstandsgang or
    /// registrierende Leistungsmessung — a precondition of the model. Without
    /// it there is nothing to allocate against.
    #[error("the Übergabestelle is not quarter-hour metered (Anlage 6 §III.1)")]
    KeineViertelstundenmessung,

    /// The conservation identity of Anlage 6 §IV.1 does not hold.
    ///
    /// Raised only where it is a *bug* — a caller that assembled a version by
    /// hand and got the arithmetic wrong. The ordinary shortfall is not this:
    /// it is the Deltamenge, which is a quantity with a Bilanzkreis of its own.
    #[error("conservation broken in {slot}: NGZ {ngz} ≠ Σ {summe} + Δ {delta}")]
    ErhaltungVerletzt {
        /// The quarter hour, as an RFC 3339 instant.
        slot: String,
        /// The Netzgangzeitreihe value.
        ngz: Decimal,
        /// What the parts add up to.
        summe: Decimal,
        /// The Deltamenge that was recorded.
        delta: Decimal,
    },

    /// A version already sealed as `Final` was edited.
    ///
    /// MaBiS Kap. 3.8.2 versions by Erstellungszeitpunkt; once a version has
    /// settled it is immutable and a correction is a *new* version.
    #[error("version {erstellungszeitpunkt} is final and cannot be edited")]
    VersionIstFinal {
        /// The sealed version's Erstellungszeitpunkt.
        erstellungszeitpunkt: String,
    },

    /// A correction arrived after the MaBiS Korrekturfrist.
    ///
    /// MaBiS Kap. 3.10 closes the window at the end of month M+7 relative to
    /// the Bilanzierungsmonat.
    #[error("corrections to Bilanzierungsmonat {monat} closed on {frist}; {eingang} is too late")]
    KorrekturfristAbgelaufen {
        /// The Bilanzierungsmonat, as `YYYY-MM`.
        monat: String,
        /// The last day corrections were accepted.
        frist: time::Date,
        /// When the correction arrived.
        eingang: time::Date,
    },

    /// Two claims in one quarter hour name the same virtual Marktlokation.
    ///
    /// Refused rather than summed. A MaLo appearing twice produces two
    /// [`crate::allocation::Zuordnung`] rows for one Bilanzkreis-Zuordnung,
    /// and every downstream consumer that keys by MaLo — the BK-SZR
    /// aggregation above all — silently keeps one of them. Merge the claims
    /// upstream, where the reason they are two is still known.
    #[error("virtual Marktlokation {malo} claims the same quarter hour twice")]
    DoppelterAnspruch {
        /// The Marktlokation that appears more than once.
        malo: String,
    },

    /// A Ladevorgang spans more quarter hours than a session can.
    ///
    /// Not a regulatory bound but a corruption guard: the split walks one
    /// slot at a time, so an `ende` that a backend got wrong by a factor of a
    /// thousand would otherwise allocate until the process dies. A year of
    /// quarter hours is already three orders of magnitude past the longest
    /// real charging session.
    #[error("session {id} spans {slots} quarter hours, more than the {max} a session may")]
    LadevorgangZuLang {
        /// The session's own id.
        id: String,
        /// How many quarter hours it would span.
        slots: u64,
        /// The most a session may span.
        max: u64,
    },

    /// The allocation could not be formed at all.
    #[error("allocation failed: {0}")]
    Allocation(String),
}

impl From<metering::allocation::AllocationError> for EmobError {
    fn from(e: metering::allocation::AllocationError) -> Self {
        Self::Allocation(e.to_string())
    }
}
