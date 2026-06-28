//! Post-acceptance obligation builders for GPKE processes.
//!
//! These helpers encode the BDEW GPKE Teil 3/4 mandatory follow-up obligations
//! that arise when an NB accepts specific inbound processes. Keeping obligation
//! construction separate from the workflow state machine allows
//! [`crate::GpkeSupplierChangeWorkflow`] to remain a pure state machine without
//! cross-process PID knowledge.
//!
//! Callers (transport adapters, integration tests, examples) invoke these
//! builders to compute the `obligations` slice passed to
//! [`crate::SupplierChangeCommand::SendAntwort`].
//!
//! # Example
//!
//! ```rust,ignore
//! use mako_gpke::post_acceptance;
//! use mako_gpke::SupplierChangeCommand;
//!
//! let obligations = post_acceptance::lieferbeginn_obligations(
//!     anfrage_pid.as_u32(),
//!     &data.location_id,
//!     &data.new_supplier,
//!     Some(&msb_gln),
//! );
//! process.execute(SupplierChangeCommand::SendAntwort {
//!     accepted: true,
//!     reason: None,
//!     obligations,
//! }).await?;
//! ```

use mako_engine::{
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode},
};

/// Build the GPKE Teil 3 + 4 post-acceptance obligations for a
/// `Lieferbeginn Strom` acceptance (anfrage PID 55001).
///
/// Returns an empty `Vec` when `anfrage_pid != 55001` — other
/// Prüfidentifikatoren do not carry Teil 3/4 obligations.
///
/// # Obligations (BDEW GPKE / BK6-22-024)
///
/// - **MSCONS 13015** — "Bewegungsdaten im Kalenderjahr vor Lieferbeginn Strom":
///   The NB delivers historical movement data to the new LFN before the supply
///   start date. Always emitted when the anfrage PID is 55001 and accepted.
///
/// - **ORDERS 17134** — "Einrichtung Konfiguration aufgrund Zuordnung LF (NB an
///   MSB)": The NB orders the MSB to configure the metering point for the new
///   supplier assignment. Emitted only when `msb_gln` is `Some`.
///
/// Both entries are intended to be passed as `obligations` to
/// [`crate::SupplierChangeCommand::SendAntwort`] so they are co-persisted
/// atomically with the `AntwortGesendet` event via the outbox.
///
/// # Parameters
///
/// - `anfrage_pid` — The Prüfidentifikator of the inbound ANFRAGE (e.g. `55001`).
/// - `malo` — The Marktlokation identifier for the delivery point.
/// - `new_supplier` — The GLN/EIC of the incoming Lieferant (LFN).
/// - `msb_gln` — The GLN/EIC of the Messstellenbetreiber (MSB). Pass `None`
///   when the MSB is unknown at dispatch time; the ORDERS 17134 obligation is
///   then omitted and must be fulfilled via a separate process.
#[must_use]
pub fn lieferbeginn_obligations(
    anfrage_pid: u32,
    malo: &MaLo,
    new_supplier: &MarktpartnerCode,
    msb_gln: Option<&MarktpartnerCode>,
) -> Vec<PendingOutbox> {
    if anfrage_pid != 55001 {
        return vec![];
    }
    let mut obligations = vec![PendingOutbox::new(
        "MSCONS",
        new_supplier.as_str(),
        serde_json::json!({
            "type":         "MovementDataRequired",
            "pid":          13015_u32,
            "malo":         malo.as_str(),
            "new_supplier": new_supplier.as_str(),
            "anfrage_pid":  anfrage_pid,
        }),
    )];
    if let Some(msb) = msb_gln {
        obligations.push(PendingOutbox::new(
            "ORDERS",
            msb.as_str(),
            serde_json::json!({
                "type":         "KonfigurationseinrichtungRequired",
                "pid":          17134_u32,
                "malo":         malo.as_str(),
                "new_supplier": new_supplier.as_str(),
                "anfrage_pid":  anfrage_pid,
            }),
        ));
    }
    obligations
}
