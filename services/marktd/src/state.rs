//! The router state every core handler receives.

use std::sync::Arc;

use mako_markt::makod_client::MakodClient;

use crate::pg::{
    PgCorrelationIndex, PgMaloRepository, PgMeloRepository, PgPartnerRepository,
    PgSubscriptionRepository,
};

/// The repositories and identity behind the core routes, passed to handlers as
/// `State<Arc<AppState>>`.
///
/// Concrete: marktd has one instantiation, the PostgreSQL repositories, and
/// every repository is exercised against real PostgreSQL rather than a test
/// double.
#[derive(Clone)]
pub struct AppState {
    /// MaLo master data.
    pub malo_repo: PgMaloRepository,
    /// MeLo master data.
    pub melo_repo: PgMeloRepository,
    /// Webhook subscriptions.
    pub subscription_repo: PgSubscriptionRepository,
    /// Process correlation index.
    pub correlation_index: PgCorrelationIndex,
    /// Market partners.
    pub partner_repo: PgPartnerRepository,
    /// Client for makod's admin API.
    pub makod_client: Arc<MakodClient>,
    /// This deployment's own tenant identifier — the operator's primary market
    /// code (`makod.toml` `[[party]] primary = true`).
    ///
    /// A BDEW- or DVGW-Codenummer, not a GS1 GLN, and validated as neither: it
    /// is compared against the caller's `mako_tenant` claim and written on
    /// tenant-scoped rows, never parsed.
    pub tenant: String,
}
