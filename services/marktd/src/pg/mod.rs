//! PostgreSQL repository implementations for `marktd`.
//!
//! Each `Pg*` struct is a thin `PgPool` wrapper.  The pool is `Clone + Send + Sync`
//! and can be passed to axum `State<Arc<AppState<...>>>` without extra `Arc` wrapping.

use mako_markt::error::MdmError;

/// PostgreSQL `exclusion_violation`.
///
/// The code an `EXCLUDE USING gist` constraint raises. `0001_initial.sql`
/// declares eight of them — `rollenzuordnungen_no_overlap`,
/// `melo_msb_no_overlap`, `nb_contracts_no_overlap` and the five
/// `preisblaetter*_no_overlap` — and every one of them states the same thing:
/// no two rows for the same party may claim the same day. Not `23514`: a check
/// constraint and an exclusion constraint are different codes, and mapping only
/// the former left every overlap answering `500`.
const EXCLUSION_VIOLATION: &str = "23P01";

/// PostgreSQL `check_violation`.
///
/// Column bounds — the `prozent` range on `lf_zuordnung`, the GPKE Teil 1
/// Tranchen bound — and the conservation trigger, which raises with this
/// `ERRCODE` on purpose.
const CHECK_VIOLATION: &str = "23514";

/// Map a failed write onto the caller's error.
///
/// Both codes state something about the *request*, not about the database: a
/// share outside „> 0 % und < 100 %", a Marktlokation split beyond the whole, a
/// price sheet whose validity window overlaps the one already stored, a
/// backdated MSB assignment that would leave two Messstellenbetreiber valid on
/// the same day. None of them is an outage, and answering `500` tells the caller
/// the server broke when what it needs to hear is which of its own dates to move.
///
/// Matched on the SQLSTATE rather than on a constraint name, so a constraint
/// added to the schema is classified correctly without being listed here — the
/// failure mode of a name list is that the newest rule is the one it misses.
///
/// Anything else stays [`MdmError::Internal`]: a connection reset or a syntax
/// error is not the caller's to fix.
pub fn write_error(e: sqlx::Error) -> MdmError {
    let sqlx::Error::Database(db) = &e else {
        return MdmError::Internal(e.to_string());
    };
    match db.code().as_deref() {
        Some(EXCLUSION_VIOLATION) => MdmError::Unprocessable {
            reason: format!(
                "{}: the row's [valid_from, valid_to) window overlaps one already stored.                  Close the existing row at the new start date, or move valid_from.",
                db.message()
            ),
        },
        Some(CHECK_VIOLATION) => MdmError::Unprocessable {
            reason: db.message().to_owned(),
        },
        _ => MdmError::Internal(e.to_string()),
    }
}

pub mod bilanzierung;
pub mod correlation;
pub mod device;
pub mod einwilligung;
pub mod grundversorger;
pub mod lokationszuordnung;
pub mod mabis_zp;
pub mod malo;
pub mod malo_grid;
pub mod melo;
pub mod melo_msb;
pub mod mmma_preise;
pub mod msb_rahmenvertrag_gas;
pub mod nb_contract;
pub mod nelo;
pub mod netzzugang;
pub mod partner;
pub mod preisblatt;
pub mod pricat;
pub mod subscription;
pub mod tranche;
pub mod versorgung;
pub mod zaehler_register;

pub use bilanzierung::PgBilanzierungRepository;
pub use correlation::PgCorrelationIndex;
pub use device::PgDeviceRepository;
pub use device::PgSteuerbareRessourceRepository;
pub use device::PgTechnischeRessourceRepository;
pub use einwilligung::PgEinwilligungRepository;
pub use grundversorger::PgGrundversorgerRepository;
pub use lokationszuordnung::PgLokationszuordnungRepository;
pub use mabis_zp::PgMabisZpRepository;
pub use malo::PgMaloRepository;
pub use malo_grid::PgMaloGridRepository;
pub use melo::PgMeloRepository;
pub use melo_msb::PgMeloMsbRepository;
pub use mmma_preise::PgMmmPreisStromRepository;
pub use mmma_preise::PgMmmaPreisGasRepository;
pub use msb_rahmenvertrag_gas::PgMsbRahmenvertragGasRepository;
pub use nb_contract::PgNbContractRepository;
pub use nelo::PgNeLoRepository;
pub use netzzugang::PgNetzzugangRepository;
pub use partner::PgPartnerRepository;
pub use preisblatt::PgPreisblattDienstleistungRepository;
pub use preisblatt::PgPreisblattHardwareRepository;
pub use preisblatt::PgPreisblattKaRepository;
pub use preisblatt::PgPreisblattMessungRepository;
pub use preisblatt::PgPreisblattRepository;
pub use pricat::PgPriCatRepository;
pub use subscription::PgSubscriptionRepository;
pub use tranche::PgTrancheRepository;
pub use versorgung::PgVersorgungsStatusRepository;
pub use zaehler_register::PgZaehlzeitRepository;
