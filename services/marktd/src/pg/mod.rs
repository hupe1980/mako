//! PostgreSQL repository implementations for `marktd`.
//!
//! Each `Pg*` struct is a thin `PgPool` wrapper.  The pool is `Clone + Send + Sync`
//! and can be passed to axum `State<Arc<AppState<...>>>` without extra `Arc` wrapping.

pub mod contract;
pub mod correlation;
pub mod device;
pub mod lokationszuordnung;
pub mod malo;
pub mod malo_grid;
pub mod melo;
pub mod mmma_preise;
pub mod nb_contract;
pub mod nelo;
pub mod partner;
pub mod preisblatt;
pub mod pricat;
pub mod subscription;
pub mod versorgung;
pub mod zaehler_register;

pub use contract::PgContractRepository;
pub use correlation::PgCorrelationIndex;
pub use device::PgDeviceRepository;
pub use device::PgSteuerbareRessourceRepository;
pub use device::PgTechnischeRessourceRepository;
pub use lokationszuordnung::PgLokationszuordnungRepository;
pub use malo::PgMaloRepository;
pub use malo_grid::PgMaloGridRepository;
pub use melo::PgMeloRepository;
pub use mmma_preise::PgMmmPreisStromRepository;
pub use mmma_preise::PgMmmaPreisGasRepository;
pub use nb_contract::PgNbContractRepository;
pub use nelo::PgNeLoRepository;
pub use partner::PgPartnerRepository;
pub use preisblatt::PgPreisblattDienstleistungRepository;
pub use preisblatt::PgPreisblattHardwareRepository;
pub use preisblatt::PgPreisblattKaRepository;
pub use preisblatt::PgPreisblattMessungRepository;
pub use preisblatt::PgPreisblattRepository;
pub use pricat::PgPriCatRepository;
pub use subscription::PgSubscriptionRepository;
pub use versorgung::PgVersorgungsStatusRepository;
pub use zaehler_register::PgZaehlzeitRepository;
