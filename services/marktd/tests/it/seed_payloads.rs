//! Guard: every request body the seeder posts to `marktd`
//! (`tools/mako-seed/world/`) deserialises into `marktd`'s request type.
//!
//! ## Why
//!
//! A seeded request body is shipped API surface: it is run by whoever evaluates the
//! platform and copied into tickets and integration guides as *"this is what a
//! request looks like"*. A body naming fields the endpoint does not have is a
//! `400` at run time (`cargo xtask check-request-bodies`); this makes it a test
//! failure instead, without a Docker daemon or a running stack.
//!
//! ## What it does not do
//!
//! It does not run the handler. Authorisation, the database and the business
//! rules are `marktd`'s integration suites' business. This answers one question
//! — *would the body deserialise?*

use marktd::handlers::{
    lokationszuordnung::UpsertEdgeRequest,
    malo::MaloUpsertRequest,
    malo_grid::PutMaloGridBody,
    melo::MeloUpsertRequest,
    melo_msb::PutMeloMsbBody,
    partner::PartnerUpsertRequest,
    preisblatt::{
        PreisblattKaUpsertRequest, PreisblattMessungUpsertRequest, PreisblattUpsertRequest,
    },
    subscription::SubscriptionUpsertRequest,
    versorgung::VersorgungsStatusUpsertRequest,
};

/// A body from the seeder's world, `tools/mako-seed/world/<file>`.
fn fixture(file: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/mako-seed/world")
        .join(file);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Deserialise `body` as `T`, or fail naming the payload.
fn accepts<T: serde::de::DeserializeOwned>(what: &str, body: serde_json::Value) -> T {
    serde_json::from_value(body).unwrap_or_else(|e| {
        panic!(
            "{what}: marktd would refuse this seeded body — {e}\n\
             A seeded body is shipped API surface: fix tools/mako-seed/world, or the endpoint."
        )
    })
}

/// The Stadtwerk's master data — the MaLos in Strom and Gas, the
/// Netzbetreiber's Preisblätter, the grid records, the Messlokation and its
/// MSB, the LF the 55001 names and the Gas NB whose NN-Rechnung is disputed.
#[test]
fn the_seeded_master_data_payloads_deserialise() {
    for file in ["malo.json", "malo-erzeugung.json", "malo-gas.json"] {
        let malo: MaloUpsertRequest = accepts("PUT /api/v1/malos/{id}", fixture(file));
        assert_eq!(
            malo.rollenzuordnung.len(),
            1,
            "{file}: the MaLo names the NB whose Zuständigkeit the auto-responder checks"
        );
    }
    for file in ["preisblatt-nb.json", "marktd-preisblatt-gas.json"] {
        let _: PreisblattUpsertRequest = accepts("PUT /api/v1/preisblaetter/{nb}", fixture(file));
    }
    let _: PreisblattMessungUpsertRequest = accepts(
        "PUT /api/v1/preisblaetter-messung/{msb}",
        fixture("marktd-preisblatt-messung.json"),
    );
    let ka: PreisblattKaUpsertRequest = accepts(
        "PUT /api/v1/preisblaetter-ka/{nb}",
        fixture("marktd-preisblatt-ka.json"),
    );
    assert_eq!(
        ka.kundengruppe_ka.as_deref(),
        Some("Tarifkunden"),
        "the Endkunden-Angebot of a household reads the Tarifkunden sheet"
    );
    let _: VersorgungsStatusUpsertRequest = accepts(
        "PUT /api/v1/versorgung/{malo}",
        fixture("marktd-versorgung.json"),
    );
    for file in ["marktd-partner-lf.json", "marktd-partner-gnb.json"] {
        let _: PartnerUpsertRequest = accepts("PUT /api/v1/partners/{mp_id}", fixture(file));
    }
    for file in ["marktd-grid.json", "marktd-grid-gas.json"] {
        let _: PutMaloGridBody = accepts("PUT /api/v1/malos/{id}/grid", fixture(file));
    }
    let _: MeloUpsertRequest = accepts("PUT /api/v1/melos/{id}", fixture("marktd-melo.json"));
    // `data: {}` stays valid: the gate injects the absent `_typ`, and every
    // `Lokationszuordnung` field is optional.
    let _: UpsertEdgeRequest = accepts(
        "PUT /api/v1/lokationszuordnungen",
        fixture("marktd-lokationszuordnung.json"),
    );
    let _: UpsertEdgeRequest = accepts(
        "PUT /api/v1/lokationszuordnungen (with a Lokationsbündelstruktur)",
        serde_json::json!({
            "von_id": "51238696781", "von_typ": "MALO",
            "nach_id": "DE0001234567890000000000000000001", "nach_typ": "MELO",
            "data": { "lokationsbuendelcode": "9992000000026" }
        }),
    );
    let _: PutMeloMsbBody = accepts(
        "PUT /api/v1/melos/{id}/msb",
        fixture("marktd-melo-msb.json"),
    );
    let _: SubscriptionUpsertRequest = accepts(
        "PUT /api/v1/subscriptions/{id}",
        serde_json::json!({
            "webhook_url": "http://127.0.0.1:9199/",
            "event_types": ["de.mako.process.initiated", "de.mako.process.completed"],
            "active": true
        }),
    );
}

/// The rule that makes the test above worth having.
///
/// Every one of these bodies denies unknown fields, so a payload naming a field
/// the endpoint does not have is a refusal rather than a silently discarded
/// value. Pinned on `channels`, which belongs to `makod`'s partner store and
/// not to `marktd`'s — the confusable name is the one worth holding.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let err = serde_json::from_value::<PartnerUpsertRequest>(serde_json::json!({
        "display_name": "Demo LF",
        "channels": [{ "qualifier": "AK", "address": "https://as4.example/in" }]
    }))
    .expect_err("`channels` is makod's partner store, not marktd's");
    assert!(
        err.to_string().contains("unknown field") && err.to_string().contains("channels"),
        "the refusal must name the field: {err}"
    );
}

/// The partner body's `mp_id` is optional, and refused when it disagrees.
///
/// The path already names the partner, so requiring the caller to repeat it
/// adds a way to be wrong and no information — the same reading the BO4E gate
/// gives `_typ`. When it *is* supplied the handler compares the two: letting
/// the path silently win means a body naming another partner is neither
/// honoured nor reported.
#[test]
fn the_partner_body_need_not_repeat_the_path() {
    let without: PartnerUpsertRequest =
        serde_json::from_value(serde_json::json!({ "display_name": "Demo LF" }))
            .expect("mp_id is optional");
    assert!(without.mp_id.is_none());

    let with: PartnerUpsertRequest =
        serde_json::from_value(serde_json::json!({ "mp_id": "4012345000023" }))
            .expect("a well-formed MP-ID is accepted");
    assert_eq!(
        with.mp_id.as_ref().map(std::string::ToString::to_string),
        Some("4012345000023".to_owned())
    );
}

/// The BO4E gate runs **inside** the body's `Deserialize`, so a payload naming
/// the wrong Business Object is refused here too — with the gate's own wording,
/// not a generic serde message.
///
/// This is the half a `deny_unknown_fields` sweep cannot cover: the key is
/// spelled right and the value is the wrong document.
#[test]
fn a_wrong_discriminator_in_an_edge_payload_is_refused() {
    let body = serde_json::json!({
        "von_id": "51238696781", "von_typ": "MALO",
        "nach_id": "DE0001234567890000000000000000001", "nach_typ": "MELO",
        "data": { "_typ": "MARKTLOKATION" }
    });
    let err = serde_json::from_value::<UpsertEdgeRequest>(body)
        .expect_err("a Marktlokation is not a Lokationszuordnung");
    assert!(
        err.to_string()
            .contains("expected a BO4E LOKATIONSZUORDNUNG"),
        "the gate's own sentence must survive: {err}"
    );
}

/// An out-of-schema enum anywhere in the document is refused by JSON-path.
///
/// Without the gate it would decode to BO4E's `Unknown` catch-all and
/// **serialise back as the literal string `"UNKNOWN"`** — so the endpoint would
/// not merely accept the typo, it would overwrite what the caller sent.
#[test]
fn an_out_of_schema_enum_in_an_edge_payload_is_refused() {
    let body = serde_json::json!({
        "von_id": "51238696781", "von_typ": "MALO",
        "nach_id": "DE0001234567890000000000000000001", "nach_typ": "MELO",
        "data": { "marktlokationen": [{ "sparte": "STROMM" }] }
    });
    let err =
        serde_json::from_value::<UpsertEdgeRequest>(body).expect_err("STROMM is not a Sparte");
    assert!(
        err.to_string().contains("out-of-schema enum"),
        "the finding must name the stage: {err}"
    );
}

/// The **per-object-code** audit runs on the write path, where the document
/// carries its objects inline. The read path's `audit_struktur` cannot see
/// object codes at all — the graph it projects from holds ids grouped by
/// `Lokationstyp` — so this is the only place an object filed under the wrong
/// type can be caught.
#[test]
fn the_write_path_can_audit_object_codes_the_graph_cannot_see() {
    use rubo4e::lokationsbuendel::LokationsbuendelExt as _;

    // `9992000000026` is *Verbrauch mit einer Messlokation (Standard)*, whose
    // Messlokation row is `9992000001032`. Putting that code on the
    // **Marktlokation** is well-formed, catalogued, and wrong.
    let body = serde_json::json!({
        "von_id": "51238696781", "von_typ": "MALO",
        "nach_id": "DE0001234567890000000000000000001", "nach_typ": "MELO",
        "data": {
            "lokationsbuendelcode": "9992000000026",
            "marktlokationen": [{
                "marktlokationsId": "51238696781",
                "lokationsbuendelObjektcode": "9992000001032"
            }],
            "messlokationen": [{
                "messlokationsId": "DE0001234567890000000000000000001",
                "lokationsbuendelObjektcode": "9992000001032"
            }]
        }
    });
    let req: UpsertEdgeRequest = accepts(
        "PUT /api/v1/lokationszuordnungen (mis-filed object code)",
        body,
    );
    let audit = req.data.expect("present").audit_buendel();
    assert!(
        !audit.is_conformant(),
        "a Messlokation code on a Marktlokation must be reported"
    );
    assert_eq!(
        audit.struktur.expect("published").bezeichnung,
        "Verbrauch mit einer Messlokation (Standard)"
    );
}

/// `data: {}` — what the seeder sends, and the shape of an edge asserted before
/// the bundle document that explains it exists — must produce **no** findings.
///
/// `rubo4e`'s audit reports `lokationsbuendelcode is absent` for an empty
/// document, which is the right answer to *which structure is this* (the read
/// path keeps it) and the wrong one to *does this match what it declares*. Left
/// in, every edge `PUT` in the tree would carry a finding and the field would
/// be noise.
#[test]
fn an_edge_with_no_bundle_document_reports_nothing() {
    use rubo4e::lokationsbuendel::{Befund, LokationsbuendelExt as _};

    let req: UpsertEdgeRequest = accepts(
        "PUT /api/v1/lokationszuordnungen (no bundle document)",
        serde_json::json!({
            "von_id": "51238696781", "von_typ": "MALO",
            "nach_id": "DE0001234567890000000000000000001", "nach_typ": "MELO",
            "data": {}
        }),
    );
    let audit = req.data.expect("present").audit_buendel();
    assert_eq!(
        audit
            .befunde
            .iter()
            .filter(|b| !matches!(b, Befund::StrukturcodeFehlt))
            .count(),
        0,
        "an empty document has nothing to depart from: {:?}",
        audit.befunde
    );
}
