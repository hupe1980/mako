//! A MaLo write in marktd calls no other service.
//!
//! makod's MaLo-Ident cache follows marktd through the durable fan-out
//! (`de.markt.malo.updated`, enqueued in the write's transaction). A call from
//! the handler after its commit could not be retried by anything: a makod that
//! was down at that moment would keep answering counterparties from a stale
//! record while marktd reported success. So the handler, and the state it
//! reads, hold no makod client at all.

const MALO_HANDLER: &str = include_str!("../../src/handlers/malo.rs");
const STATE: &str = include_str!("../../src/state.rs");

/// The fragments a call into makod's admin API needs.
const MAKOD_CALL: &[&str] = &["makod_client", "MakodClient", "/admin/malo"];

#[test]
fn the_malo_handler_makes_no_call_to_makod() {
    for needle in MAKOD_CALL {
        assert!(
            !MALO_HANDLER.contains(needle),
            "services/marktd/src/handlers/malo.rs names {needle:?}: a MaLo write must \
             reach makod through the fan-out, never by a call after its commit"
        );
        assert!(
            !STATE.contains(needle),
            "AppState names {needle:?}: no core handler may call makod"
        );
    }
}

#[test]
fn the_malo_handler_still_enqueues_the_event_makod_follows() {
    assert!(
        MALO_HANDLER.contains("mako_events::markt::MALO_UPDATED")
            && MALO_HANDLER.contains("outbox::enqueue"),
        "put_malo must enqueue de.markt.malo.updated in its transaction — it is the only \
         way makod's cache learns of the change"
    );
}
