//! An online order (`POST /api/v1/bestellungen`) and its Bestellnachweis — the
//! pure half: the request body, its completeness, the label, the canonical
//! hash and the idempotency comparison.
//!
//! § 312j Abs. 4 BGB: „Ein Vertrag nach Absatz 2 kommt nur zustande, wenn der
//! Unternehmer seine Pflicht aus Absatz 3 erfüllt." In this channel the record
//! of the order *is* the evidence that a contract exists, so an order that
//! cannot be recorded completely is refused before anything is stored, naming
//! every missing element.

use mako_markt::bo4e::Bo4e;
use rubo4e::current::Geschaeftspartner;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::Lieferbeginnanlass;

// ── The request body ──────────────────────────────────────────────────────────

/// `POST /api/v1/bestellungen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BestellungInput {
    /// The front end's or partner's own reference for this order — the
    /// `subject` of every `de.mako.vertrag.bestellung.*` and
    /// `de.mako.vertrag.widerruf.*` CloudEvent, so the caller correlates without
    /// polling.
    pub bestellreferenz: String,
    /// The frozen Endkunden-Angebot accepted, by id and content hash.
    pub angebot_ref: AngebotRef,
    /// The ordering customer.
    pub kunde: BestellKunde,
    /// § 13 BGB — the declared fact on the order. `true` derives the
    /// Widerrufsrecht and the Vertragsbestätigung; mako does not second-guess it.
    pub verbraucher: bool,
    pub gewuenschter_lieferbeginn: Date,
    /// The facts the Anmeldung (55001 / 44001) carries beside the MaLo.
    pub anmeldung: AnmeldeAngaben,
    /// The agreed notice period in months, as the offer page stated it.
    pub kuendigungsfrist_monate: i32,
    /// § 40b EnWG billing cadence; `JAEHRLICH` when absent.
    #[serde(default)]
    pub abrechnungszyklus: Option<String>,
    /// The § 312j Abs. 2 BGB information as displayed immediately before the
    /// order.
    pub angezeigte_informationen: AngezeigteInformationen,
    /// The order button's label (§ 312j Abs. 3 BGB).
    pub button_beschriftung: String,
    /// The documents shown, by SHA-256.
    pub dokumente: Vec<DokumentRef>,
    /// Art. 246a § 1 Abs. 2 Satz 1 Nr. 1 EGBGB — the Widerrufsbelehrung was
    /// displayed before the order.
    pub belehrung_angezeigt: bool,
    /// Art. 246a § 1 Abs. 2 Satz 1 Nr. 3 EGBGB — the Wertersatz information
    /// was displayed before the order.
    pub wertersatz_info_angezeigt: bool,
    /// § 357a Abs. 2 Satz 1 Nr. 1 BGB — the express request to start supply
    /// inside the Widerrufsfrist.
    pub lieferung_in_widerrufsfrist_verlangt: Verlangen,
    /// The click, as the front end reports it.
    #[serde(with = "time::serde::rfc3339")]
    pub bestellt_am: OffsetDateTime,
    pub kanal: Kanal,
    /// The § 14a EnWG modules the customer chose (Strom only), recorded from
    /// the Lieferbeginn.
    #[serde(default)]
    pub sect14a: Option<crate::pg::sect14a::Sect14aIntake>,
    /// The customer's previous supplier.
    #[serde(default)]
    pub vorversorger: Option<crate::kuendigung_vorversorger::Vorversorger>,
    /// Terminate the old contract on the customer's behalf before the
    /// Anmeldung (55016 / 44016).
    #[serde(default)]
    pub kuendigung_beim_vorversorger:
        Option<crate::kuendigung_vorversorger::KuendigungBeimVorversorger>,
}

/// The offer an order accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AngebotRef {
    pub id: Uuid,
    pub sha256: String,
}

/// The ordering customer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BestellKunde {
    /// An existing Kunde of this deployment the order is for — the `kunden_id`
    /// an earlier order's answer returned. Absent, the order creates a Kunde.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kunden_id: Option<uuid::Uuid>,
    /// `B2C` | `B2B_SLP` | `B2B_RLM` | `B2B_HV`.
    pub kundentyp: String,
    /// § 3 Nr. 57 EnWG; defaults to `kundentyp == "B2C"`.
    #[serde(default)]
    pub haushaltskunde: Option<bool>,
    /// Name and address as a BO4E `Geschaeftspartner` — what the Anmeldung's
    /// `SG12 NAD+Z09` names and every document is addressed to.
    pub geschaeftspartner: Bo4e<Geschaeftspartner>,
    /// The electronic address for the documents and the § 356a Abs. 4 BGB
    /// Eingangsbestätigung.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub telefon: Option<String>,
    /// The Marktlokation; must be the offer's.
    #[serde(default)]
    pub malo_id: Option<String>,
    #[serde(default)]
    pub zaehlernummer: Option<String>,
}

/// The facts of the Anmeldung the contract will carry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnmeldeAngaben {
    #[serde(default)]
    pub lieferbeginn_anlass: Option<Lieferbeginnanlass>,
    #[serde(default)]
    pub bilanzkreis: Option<String>,
    #[serde(default)]
    pub enfg_grundlage: Option<String>,
    #[serde(default)]
    pub enfg_grund: Option<String>,
    #[serde(default)]
    pub netznutzungsvertrag: Option<mako_markt::utilmd_stammdaten::Netznutzungsvertrag>,
    #[serde(default)]
    pub netznutzung_zahler: Option<mako_markt::utilmd_stammdaten::NetznutzungZahler>,
    #[serde(default)]
    pub gemeinderabatt_prozent: Option<rust_decimal::Decimal>,
}

/// The information § 312j Abs. 2 BGB requires „unmittelbar bevor der
/// Verbraucher seine Bestellung abgibt", by the Art. 246a § 1 Abs. 1 Satz 1
/// EGBGB number it answers. Each is optional on the wire so that a missing one
/// is named rather than refused as a parse error. Nr. 11a concerns a
/// manufacturer's durability guarantee for a *Ware* and has no
/// energy-supply content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AngezeigteInformationen {
    /// Nr. 1 — die wesentlichen Eigenschaften.
    #[serde(default)]
    pub wesentliche_eigenschaften: Option<String>,
    /// Nr. 5 — der Gesamtpreis.
    #[serde(default)]
    pub gesamtpreis: Option<String>,
    /// Nr. 6 — whether the price was personalised.
    #[serde(default)]
    pub personalisierter_preis: Option<bool>,
    /// Nr. 7 — zusätzliche Kosten.
    #[serde(default)]
    pub zusaetzliche_kosten: Option<String>,
    /// Nr. 8 — the total per Abrechnungszeitraum of an unbefristeter Vertrag.
    #[serde(default)]
    pub gesamtpreis_pro_abrechnungszeitraum: Option<String>,
    /// Nr. 14 — Laufzeit und Kündigung.
    #[serde(default)]
    pub laufzeit_kuendigung: Option<String>,
    /// Nr. 15 — Mindestdauer der Verpflichtungen.
    #[serde(default)]
    pub mindestdauer: Option<String>,
    /// SHA-256 of the rendered block as shown.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// One document shown before the order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DokumentRef {
    pub art: DokumentArt,
    pub sha256: String,
}

/// The documents an order cites. `PREISBLATT` resolves to the offer's product
/// version in `productd`, every other one to a `rechtsdokumente` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DokumentArt {
    Agb,
    Preisblatt,
    Widerrufsbelehrung,
    MusterWiderrufsformular,
    Datenschutz,
}

impl DokumentArt {
    /// The arts `rechtsdokumente.art` holds — every one but `PREISBLATT`.
    pub const RECHTSDOKUMENTE: [Self; 4] = [
        Self::Agb,
        Self::Widerrufsbelehrung,
        Self::MusterWiderrufsformular,
        Self::Datenschutz,
    ];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Agb => "AGB",
            Self::Preisblatt => "PREISBLATT",
            Self::Widerrufsbelehrung => "WIDERRUFSBELEHRUNG",
            Self::MusterWiderrufsformular => "MUSTER_WIDERRUFSFORMULAR",
            Self::Datenschutz => "DATENSCHUTZ",
        }
    }

    /// The `rechtsdokumente.art` value, or `None` for `PREISBLATT`.
    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        Self::RECHTSDOKUMENTE.into_iter().find(|a| a.as_db() == s)
    }
}

/// § 357a Abs. 2 Satz 1 Nr. 1 BGB, as the consumer confirmed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verlangen {
    pub verlangt: bool,
    /// SHA-256 of the text the consumer confirmed; required when `verlangt`.
    #[serde(default)]
    pub text_sha256: Option<String>,
}

/// Where the order was placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kanal {
    Website,
    PortalPartner,
    Portal,
}

impl Kanal {
    pub const ALL: [Self; 3] = [Self::Website, Self::PortalPartner, Self::Portal];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Website => "WEBSITE",
            Self::PortalPartner => "PORTAL_PARTNER",
            Self::Portal => "PORTAL",
        }
    }

    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_db() == s)
    }
}

// ── Checks ────────────────────────────────────────────────────────────────────

/// Every element the Bestellnachweis must hold and the order does not carry,
/// by its JSON path. Empty when the order is complete.
#[must_use]
pub fn fehlende_angaben(input: &BestellungInput) -> Vec<String> {
    let mut fehlend = Vec::new();
    let leer = |s: &str| s.trim().is_empty();
    let text = |v: &Option<String>| v.as_deref().is_none_or(leer);
    if leer(&input.bestellreferenz) {
        fehlend.push("bestellreferenz".to_owned());
    }
    if leer(&input.angebot_ref.sha256) {
        fehlend.push("angebot_ref.sha256".to_owned());
    }
    let i = &input.angezeigte_informationen;
    for (feld, fehlt) in [
        (
            "wesentliche_eigenschaften",
            text(&i.wesentliche_eigenschaften),
        ),
        ("gesamtpreis", text(&i.gesamtpreis)),
        ("personalisierter_preis", i.personalisierter_preis.is_none()),
        ("zusaetzliche_kosten", text(&i.zusaetzliche_kosten)),
        (
            "gesamtpreis_pro_abrechnungszeitraum",
            text(&i.gesamtpreis_pro_abrechnungszeitraum),
        ),
        ("laufzeit_kuendigung", text(&i.laufzeit_kuendigung)),
        ("mindestdauer", text(&i.mindestdauer)),
        ("sha256", text(&i.sha256)),
    ] {
        if fehlt {
            fehlend.push(format!("angezeigte_informationen.{feld}"));
        }
    }
    if leer(&input.button_beschriftung) {
        fehlend.push("button_beschriftung".to_owned());
    }
    for art in pflichtdokumente(input.verbraucher) {
        if !input.dokumente.iter().any(|d| d.art == *art) {
            fehlend.push(format!("dokumente[{}]", art.as_db()));
        }
    }
    if input.verbraucher {
        // Art. 246a § 1 Abs. 2 Satz 1 Nr. 1 and Nr. 3 EGBGB are owed before
        // the order. `false` is a recorded fact, not a missing one — it moves
        // the Frist's start to the Vertragsbestätigung and withholds Wertersatz
        // — so it is not refused here.
        if input.kunde.email.as_deref().is_none_or(leer) {
            // § 356a Abs. 2 BGB: the Widerruf names the electronic address the
            // Eingangsbestätigung goes to; the Vertragsbestätigung needs one
            // as its durable medium.
            fehlend.push("kunde.email".to_owned());
        }
    }
    if input.lieferung_in_widerrufsfrist_verlangt.verlangt
        && input
            .lieferung_in_widerrufsfrist_verlangt
            .text_sha256
            .as_deref()
            .is_none_or(leer)
    {
        fehlend.push("lieferung_in_widerrufsfrist_verlangt.text_sha256".to_owned());
    }
    if input.kuendigungsfrist_monate < 0 {
        fehlend.push("kuendigungsfrist_monate (≥ 0)".to_owned());
    }
    fehlend
}

/// The documents an order must cite: the AGB, the Preisblatt and the
/// Datenschutzhinweise always; the Widerrufsbelehrung and the
/// Muster-Widerrufsformular for a consumer.
#[must_use]
pub const fn pflichtdokumente(verbraucher: bool) -> &'static [DokumentArt] {
    if verbraucher {
        &[
            DokumentArt::Agb,
            DokumentArt::Preisblatt,
            DokumentArt::Widerrufsbelehrung,
            DokumentArt::MusterWiderrufsformular,
            DokumentArt::Datenschutz,
        ]
    } else {
        &[
            DokumentArt::Agb,
            DokumentArt::Preisblatt,
            DokumentArt::Datenschutz,
        ]
    }
}

/// Whether `label` is admitted: the statutory wording, or one the operator
/// lists under `[policy] bestellbutton_beschriftungen`. Compared exactly — a
/// label is what the consumer read, and „Zahlungspflichtig Bestellen!" is not
/// the wording an operator approved.
#[must_use]
pub fn beschriftung_zulaessig(label: &str, zugelassen: &[String]) -> bool {
    zugelassen.iter().any(|z| z == label)
}

// ── Canonical hash ────────────────────────────────────────────────────────────

/// RFC 8785 (JSON Canonicalization Scheme) bytes of `value` and their SHA-256,
/// lower-case hex — the form a third party re-serialising the record with
/// another library reproduces.
///
/// # Errors
///
/// When `value` cannot be serialised (a non-finite number).
pub fn kanonisch(value: &serde_json::Value) -> Result<(Vec<u8>, String), serde_json::Error> {
    let bytes = serde_jcs::to_vec(value)?;
    let hash = sha256_hex(&bytes);
    Ok((bytes, hash))
}

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── Idempotency ───────────────────────────────────────────────────────────────

/// What a request carrying an `Idempotency-Key` already seen is.
///
/// The comparison is the IETF `Idempotency-Key` header draft's
/// (draft-ietf-httpapi-idempotency-key-header): the same key with the same
/// request is the first request again and gets the first answer; the same key
/// with another request is a client error. Kept to one function so the shared
/// idempotency layer can lift it unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wiederholung {
    /// Same key, same body: answer with the stored first response.
    Replay,
    /// Same key, other body: `422 idempotency_key_reused`.
    KeyReused,
}

/// Compare a repeated request's body hash with the stored one.
#[must_use]
pub fn wiederholung(gespeichert_sha256: &str, anfrage_sha256: &str) -> Wiederholung {
    if gespeichert_sha256 == anfrage_sha256 {
        Wiederholung::Replay
    } else {
        Wiederholung::KeyReused
    }
}

/// The stable codes of the idempotency contract.
pub mod idempotency_codes {
    /// No `Idempotency-Key` header: `400`.
    pub const MISSING: &str = "idempotency_key_missing";
    /// Same key, different body: `422`.
    pub const REUSED: &str = "idempotency_key_reused";
    /// A request with the same key is still being processed: `409`.
    pub const IN_FLIGHT: &str = "idempotency_request_in_flight";
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A complete consumer order, as the contract in `contracts/rest.md`.
    pub(crate) fn input() -> BestellungInput {
        serde_json::from_value(serde_json::json!({
            "bestellreferenz": "WEB-2026-000123",
            "angebot_ref": { "id": Uuid::nil(), "sha256": "ab".repeat(32) },
            "kunde": {
                "kundentyp": "B2C",
                "geschaeftspartner": {
                    "_typ": "GESCHAEFTSPARTNER",
                    "vorname": "Erika",
                    "nachname": "Mustermann",
                    "adresse": {
                        "strasse": "Hauptstraße", "hausnummer": "1",
                        "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
                    }
                },
                "email": "erika@example.org",
                "malo_id": "51238696012"
            },
            "verbraucher": true,
            "gewuenschter_lieferbeginn": "2027-02-01",
            "anmeldung": { "lieferbeginn_anlass": "WECHSEL", "enfg_grundlage": "ZG1" },
            "kuendigungsfrist_monate": 1,
            "angezeigte_informationen": {
                "wesentliche_eigenschaften": "Ökostrom",
                "gesamtpreis": "1.234,56 EUR/Jahr",
                "personalisierter_preis": false,
                "zusaetzliche_kosten": "keine",
                "gesamtpreis_pro_abrechnungszeitraum": "102,88 EUR/Monat",
                "laufzeit_kuendigung": "unbefristet, 1 Monat",
                "mindestdauer": "keine",
                "sha256": "cd".repeat(32)
            },
            "button_beschriftung": "zahlungspflichtig bestellen",
            "dokumente": [
                { "art": "AGB", "sha256": "01".repeat(32) },
                { "art": "PREISBLATT", "sha256": "02".repeat(32) },
                { "art": "WIDERRUFSBELEHRUNG", "sha256": "03".repeat(32) },
                { "art": "MUSTER_WIDERRUFSFORMULAR", "sha256": "04".repeat(32) },
                { "art": "DATENSCHUTZ", "sha256": "05".repeat(32) }
            ],
            "belehrung_angezeigt": true,
            "wertersatz_info_angezeigt": true,
            "lieferung_in_widerrufsfrist_verlangt": { "verlangt": true, "text_sha256": "06".repeat(32) },
            "bestellt_am": "2026-11-15T19:02:11+01:00",
            "kanal": "WEBSITE"
        }))
        .expect("the fixture order parses")
    }

    #[test]
    fn a_complete_order_misses_nothing() {
        assert!(fehlende_angaben(&input()).is_empty());
    }

    /// Every missing element is named, not the first.
    #[test]
    fn an_incomplete_order_names_every_missing_element() {
        let mut i = input();
        i.angezeigte_informationen.gesamtpreis = None;
        i.angezeigte_informationen.mindestdauer = Some("  ".to_owned());
        i.dokumente
            .retain(|d| d.art != DokumentArt::MusterWiderrufsformular);
        i.lieferung_in_widerrufsfrist_verlangt.text_sha256 = None;
        assert_eq!(
            fehlende_angaben(&i),
            [
                "angezeigte_informationen.gesamtpreis",
                "angezeigte_informationen.mindestdauer",
                "dokumente[MUSTER_WIDERRUFSFORMULAR]",
                "lieferung_in_widerrufsfrist_verlangt.text_sha256",
            ]
        );
    }

    /// A business customer is owed no Widerrufsbelehrung, so its absence is
    /// not missing.
    #[test]
    fn a_business_order_needs_no_widerrufsbelehrung() {
        let mut i = input();
        i.verbraucher = false;
        i.dokumente.retain(|d| {
            !matches!(
                d.art,
                DokumentArt::Widerrufsbelehrung | DokumentArt::MusterWiderrufsformular
            )
        });
        assert!(fehlende_angaben(&i).is_empty());
    }

    /// § 312j Abs. 3 BGB: the statutory wording by default, nothing else.
    #[test]
    fn a_label_other_than_the_statutory_one_is_refused_by_default() {
        let default = vec![crate::config::BESTELLBUTTON_GESETZLICH.to_owned()];
        assert!(beschriftung_zulaessig(
            "zahlungspflichtig bestellen",
            &default
        ));
        assert!(!beschriftung_zulaessig("Weiter", &default));
        assert!(!beschriftung_zulaessig(
            "Zahlungspflichtig bestellen",
            &default
        ));
        let listed = vec!["kaufen".to_owned()];
        assert!(beschriftung_zulaessig("kaufen", &listed));
    }

    /// RFC 8785: key order and whitespace do not change the hash, a value does.
    #[test]
    fn the_hash_is_independent_of_key_order() {
        let a = serde_json::json!({ "b": 1, "a": "x" });
        let b: serde_json::Value = serde_json::from_str(r#"{ "a" : "x",  "b":1 }"#).unwrap();
        assert_eq!(kanonisch(&a).unwrap(), kanonisch(&b).unwrap());
        assert_eq!(kanonisch(&a).unwrap().0, br#"{"a":"x","b":1}"#);
        let c = serde_json::json!({ "b": 2, "a": "x" });
        assert_ne!(kanonisch(&a).unwrap().1, kanonisch(&c).unwrap().1);
    }

    #[test]
    fn the_same_key_with_another_body_is_refused() {
        assert_eq!(wiederholung("aa", "aa"), Wiederholung::Replay);
        assert_eq!(wiederholung("aa", "bb"), Wiederholung::KeyReused);
    }

    #[test]
    fn a_dokument_art_round_trips_its_column_value() {
        for a in DokumentArt::RECHTSDOKUMENTE {
            assert_eq!(DokumentArt::from_db(a.as_db()), Some(a));
        }
        assert_eq!(DokumentArt::from_db("PREISBLATT"), None);
    }
}
