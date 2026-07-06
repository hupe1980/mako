//! PostgreSQL repository implementations for `mdmd`.
//!
//! Each `Pg*` struct is a thin `PgPool` wrapper.  The pool is `Clone + Send + Sync`
//! and can be passed to axum `State<Arc<AppState<...>>>` without extra `Arc` wrapping.

pub mod contract;
pub mod correlation;
pub mod malo;
pub mod melo;
pub mod partner;
pub mod subscription;

pub use contract::PgContractRepository;
pub use correlation::PgCorrelationIndex;
pub use malo::PgMaloRepository;
pub use melo::PgMeloRepository;
pub use partner::PgPartnerRepository;
pub use subscription::PgSubscriptionRepository;
