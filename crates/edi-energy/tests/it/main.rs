//! Integration tests of `edi-energy`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod answer_pid_names;
mod builder_writer_guard;
mod charset;
mod conformance;
mod demo_fixtures;
mod element_positions;
mod extraction_fidelity;
mod fixture_placement;
mod fuzz_compat;
mod hierarchy;
mod interchange_party_identity;
mod layout_audit;
mod orders_sperrung_profile;
mod parse;
mod party_agency_code;
#[cfg(all(
    feature = "mscons",
    feature = "reqote",
    feature = "quotes",
    feature = "orders",
    feature = "ordchg",
    feature = "ordrsp"
))]
mod pid_detection;
mod registry;
mod roundtrip;
mod segment_layout_guard;
mod semantic;
mod skeletons;
mod typed_fields;
mod types;
mod validation_snapshot;
