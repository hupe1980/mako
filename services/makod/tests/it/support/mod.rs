//! Shared fixtures for the renderer conformance tests.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::party_registry::MpIdRegistry;

/// A registry with one primary party in the NB role.
pub fn registry(mp_id: &str) -> MpIdRegistry {
    let party = PartyConfig {
        mp_id: mp_id.to_owned(),
        roles: vec!["NB".to_owned()],
        primary: true,
        agency: None,
        as4: None,
    };
    MpIdRegistry::from_config(&[party]).expect("test registry")
}

/// An outbox message carrying `payload` for `recipient`.
pub fn outbox_message(
    message_type: &str,
    recipient: &str,
    payload: serde_json::Value,
) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/conformance"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        message_type,
        recipient,
        payload,
    )
}

// ── API-Webdienste content signature ─────────────────────────────────────────

/// An API-Webdienste caller's signing key, for a request every test signs the
/// way a counterparty does (Regelungen zum Übertragungsweg für API-Webdienste
/// 1.2, Kap. 3.3).
pub const WEBDIENSTE_CALLER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgckWBECvjQWtsSdvR
HoiIfDEkbWbAnYri4WAvp2St+v+hRANCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/a
r/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPr
-----END PRIVATE KEY-----";

/// A self-signed P-256 certificate over [`WEBDIENSTE_CALLER_KEY`]'s public key, DER in
/// standard Base64.
pub const WEBDIENSTE_CALLER_CERT: &str = "MIIBhDCCASugAwIBAgIUd0O+Mx2XBxfhEKRqlUdnmSSHU2UwCgYIKoZIzj0EAwIwGDEWMBQGA1UEAwwNOTkwMDAwMTAwMDAwMTAeFw0yNjEwMDUxNDIzMTlaFw0zNjEwMDIxNDIzMTlaMBgxFjAUBgNVBAMMDTk5MDAwMDEwMDAwMDEwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/ar/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPro1MwUTAdBgNVHQ4EFgQUswL8Izc4762WL12or5ykXpaTBJAwHwYDVR0jBBgwFoAUswL8Izc4762WL12or5ykXpaTBJAwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNHADBEAiBxBiWnEciLutLqz4JaEIF6cpxmbA9BWAKi5qJSxOcTKwIgYUlfsyMjm3jaGMbpPZdu1Br5vFG97Ob7Kwj1Qcc7cyk=";

/// `req` signed with [`WEBDIENSTE_CALLER_KEY`] as an API-Webdienste
/// counterparty signs it: over `https://{Host}{path?query}`, the RFC 8785
/// payload and the `creationDateTime` / `transactionId` headers, which are
/// added when the request does not carry them.
pub async fn webdienste_signed(
    req: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    use base64::Engine as _;
    use energy_api::transport::content_security::{self, SignedInputs};
    let (mut parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.expect("body");
    let headers = &mut parts.headers;
    for (name, default) in [
        ("host", "msb.example.de"),
        ("creationDateTime", "2026-09-01T10:00:00.000Z"),
        ("transactionId", "f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
    ] {
        if !headers.contains_key(name) {
            headers.insert(name, default.parse().expect("header"));
        }
    }
    let text = |name: &str| headers[name].to_str().expect("ascii header").to_owned();
    let (host, created, tx) = (
        text("host"),
        text("creationDateTime"),
        text("transactionId"),
    );
    let payload = if bytes.is_empty() {
        Vec::new()
    } else {
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
        content_security::canonical_json(&json).expect("canonical")
    };
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let key = content_security::signing_key_from_pem(WEBDIENSTE_CALLER_KEY).expect("test key");
    let (digest, signature) = content_security::sign_request(
        &SignedInputs {
            uri: &format!("https://{host}{path}"),
            payload: &payload,
            creation_date_time: &created,
            transaction_id: &tx,
            initial_transaction_id: None,
        },
        &key,
    )
    .expect("sign");
    let der = base64::engine::general_purpose::STANDARD
        .decode(WEBDIENSTE_CALLER_CERT)
        .expect("fixture");
    let headers = &mut parts.headers;
    headers.insert(
        content_security::HEADER_DIGEST,
        digest.parse().expect("header"),
    );
    headers.insert(
        content_security::HEADER_SIGNATURE,
        signature.parse().expect("header"),
    );
    headers.insert(
        content_security::HEADER_CERT,
        content_security::cert_header_value(&der)
            .parse()
            .expect("header"),
    );
    axum::http::Request::from_parts(parts, axum::body::Body::from(bytes))
}

/// A Messlokation of the Gas fixtures' Marktlokation — a Zählpunktbezeichnung, `LOC+172` DE 3225
/// Bedingung `[951]`.
pub const GAS_MELO: &str = "DE0000000000000000000000000000042";

/// A Messlokation of the Strom fixtures' Marktlokation.
pub const STROM_MELO: &str = "DE0000000000000000000000000000043";

/// The Marktgebiet the Gas fixtures' Marktlokation lies in (an Area EIC).
pub const MARKTGEBIET: &str = "37Y005053MH0000R";

fn adresse(strasse: &str, hausnummer: &str) -> mako_markt::utilmd_stammdaten::Anschrift {
    mako_markt::utilmd_stammdaten::Anschrift {
        strasse: Some(strasse.to_owned()),
        hausnummer: Some(hausnummer.to_owned()),
        postleitzahl: Some("10115".to_owned()),
        ort: Some("Berlin".to_owned()),
        ortsteil: None,
        landescode: Some("DE".to_owned()),
    }
}

fn kunde() -> mako_markt::utilmd_stammdaten::Kunde {
    mako_markt::utilmd_stammdaten::Kunde {
        nachname: Some("Muster".to_owned()),
        vorname: Some("Erika".to_owned()),
        organisationsname: None,
        anschrift: Some(adresse("Postweg", "7")),
    }
}

fn netznutzung() -> mako_markt::utilmd_stammdaten::Netznutzungsdaten {
    use mako_markt::utilmd_stammdaten::{Abrechnungsvariante, Betriebszustand};
    mako_markt::utilmd_stammdaten::Netznutzungsdaten {
        valid_from: time::macros::date!(2026 - 01 - 01),
        valid_to: None,
        haushaltskunde: Some(false),
        betriebszustand: Some(Betriebszustand::Regelbetrieb),
        abrechnungstermin: Some("1001".to_owned()),
        naechste_abrechnung_jahr: Some(2027),
        abrechnungsintervall_monate: Some(12),
        abrechnungsvariante: Some(Abrechnungsvariante::ArbeitspreisLeistungspreis),
        gemeinderabatt_prozent: Some(rust_decimal::Decimal::ZERO),
    }
}

fn messlokation(
    melo: &str,
    msb: &str,
    zaehler: mako_markt::utilmd_stammdaten::ZaehlerStammdaten,
) -> mako_markt::utilmd_stammdaten::MesslokationStammdaten {
    use mako_markt::utilmd_stammdaten::{Messstellenbetrieb, MsbGrundlage, MsbRolle};
    mako_markt::utilmd_stammdaten::MesslokationStammdaten {
        melo_id: melo.to_owned(),
        messstellenbetrieb: Some(Messstellenbetrieb {
            msb_mp_id: msb.to_owned(),
            rolle: Some(MsbRolle::Grundzustaendig),
            grundlage: Some(MsbGrundlage::Vertrag),
            gmsb_mp_id: Some(msb.to_owned()),
        }),
        zaehler: vec![zaehler],
    }
}

/// `malo`'s UTILMD Stammdaten as marktd serves them for a Gas Marktlokation
/// on 2026-12-01: an RLM Marktlokation in the Niederdruck with one
/// Messlokation, its MSB `msb` and one Balgengaszähler.
pub fn gas_stammdaten(
    malo: &str,
    msb: &str,
) -> mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten {
    use mako_markt::utilmd_stammdaten::{BilanzierungStammdaten, MaloZaehlwerk, ZaehlerStammdaten};
    use rubo4e::current::{
        AbgabeArt, Fallgruppenzuordnung, Gasqualitaet, Netzebene, Prognosegrundlage,
        Zaehlergroesse, Zaehlertyp,
    };
    mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten {
        malo_id: malo.to_owned(),
        sparte: mako_markt::domain::Sparte::Gas,
        stichtag: time::macros::date!(2026 - 12 - 01),
        netzebene: Some(Netzebene::Nd),
        gasqualitaet: Some(Gasqualitaet::HGas),
        marktgebiet: Some(MARKTGEBIET.to_owned()),
        anschrift: Some(adresse("Musterstr.", "1")),
        kunde: Some(kunde()),
        zaehlwerke: vec![MaloZaehlwerk {
            obis_kennzahl: "7-20:99.33.17".to_owned(),
            konzessionsabgabe_satz: Some(AbgabeArt::Sa),
            konzessionsabgabe_eur_kwh: None,
        }],
        bilanzierung: Some(BilanzierungStammdaten {
            prognosegrundlage: Some(Prognosegrundlage::Werte),
            profiltypen: vec![],
            jahresverbrauchsprognose_kwh: None,
            fallgruppe: Some(Fallgruppenzuordnung::GabiRlmMitTagesband),
        }),
        netznutzung: Some(netznutzung()),
        messlokationen: vec![messlokation(
            GAS_MELO,
            msb,
            ZaehlerStammdaten {
                zaehler_id: "Z-GAS-1".to_owned(),
                zaehlernummer: Some("1ABC0012345678".to_owned()),
                zaehlertyp: Some(Zaehlertyp::Balgengaszaehler),
                zaehlergroesse: Some(Zaehlergroesse::G4),
                fernauslesbar: Some(false),
                obis_kennzahlen: vec!["7-20:99.33.17".to_owned()],
            },
        )],
    }
}

/// `malo`'s UTILMD Stammdaten as marktd serves them for a Strom Marktlokation
/// on 2026-12-01: an SLP Marktlokation in the Niederspannung with one
/// Messlokation, its MSB `msb` and one moderne Messeinrichtung.
pub fn strom_stammdaten(
    malo: &str,
    msb: &str,
) -> mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten {
    use mako_markt::utilmd_stammdaten::{BilanzierungStammdaten, ZaehlerStammdaten};
    use rubo4e::current::{Netzebene, Profiltyp, Prognosegrundlage, Zaehlertyp};
    mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten {
        malo_id: malo.to_owned(),
        sparte: mako_markt::domain::Sparte::Strom,
        stichtag: time::macros::date!(2026 - 12 - 01),
        netzebene: Some(Netzebene::Nsp),
        gasqualitaet: None,
        marktgebiet: None,
        anschrift: Some(adresse("Musterstr.", "1")),
        kunde: Some(kunde()),
        zaehlwerke: vec![],
        bilanzierung: Some(BilanzierungStammdaten {
            prognosegrundlage: Some(Prognosegrundlage::Profile),
            profiltypen: vec![Profiltyp::SlpSep],
            jahresverbrauchsprognose_kwh: Some(rust_decimal::Decimal::from(3500)),
            fallgruppe: None,
        }),
        netznutzung: Some(netznutzung()),
        messlokationen: vec![messlokation(
            STROM_MELO,
            msb,
            ZaehlerStammdaten {
                zaehler_id: "Z-STROM-1".to_owned(),
                zaehlernummer: Some("1EMH0012345678".to_owned()),
                zaehlertyp: Some(Zaehlertyp::ModerneMesseinrichtung),
                zaehlergroesse: None,
                fernauslesbar: Some(false),
                obis_kennzahlen: vec!["1-1:1.8.0".to_owned()],
            },
        )],
    }
}

/// A stand-in marktd serving `record` as the UTILMD Stammdaten of whichever
/// Marktlokation is asked for — `None` answers 404, as marktd does for a
/// Marktlokation it does not hold.
pub async fn stub_marktd(
    record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>,
) -> mako_markt::marktd_client::MarktdClient {
    use axum::{extract::Path, response::IntoResponse, routing::get};
    let record = std::sync::Arc::new(record);
    let app = axum::Router::new().route(
        "/api/v1/malos/{id}/utilmd-stammdaten",
        get(move |Path(id): Path<String>| {
            let record = std::sync::Arc::clone(&record);
            async move {
                match record.as_ref() {
                    Some(r) => {
                        let mut r = r.clone();
                        r.malo_id = id;
                        axum::Json(r).into_response()
                    }
                    None => axum::http::StatusCode::NOT_FOUND.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stand-in marktd");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    mako_markt::marktd_client::MarktdClient::new(
        format!("http://{addr}"),
        secrecy::SecretString::from("test-key"),
        reqwest::Client::new(),
    )
}

/// A marktd client pointed at a port nothing listens on.
pub fn unreachable_marktd() -> mako_markt::marktd_client::MarktdClient {
    mako_markt::marktd_client::MarktdClient::new(
        "http://127.0.0.1:9",
        secrecy::SecretString::from("test-key"),
        reqwest::Client::new(),
    )
}

/// The `geli.eog.anmelden` body for `malo` to the E/G `gv_mp_id` — the
/// command's facts only; makod reads the Marktlokation's Stammdaten from
/// marktd.
pub fn geli_eog_anmeldung(malo: &str, gv_mp_id: &str) -> serde_json::Value {
    serde_json::json!({
        "malo_id": malo,
        "gv_mp_id": gv_mp_id,
        "process_date": "2026-12-01",
        "bilanzierungsbeginn": "2026-12-01",
        "transaktionsgrund": "Z36",
    })
}
