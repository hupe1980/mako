//! Contract lifecycle and order payloads that subscribers receive.

use uuid::Uuid;

use crate::{dokumente::PreisPosition, domain::Kuendigungsgrund, pg::produkte::Initiator};

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Aktiv {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct KuendigungWiderrufen {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Bestaetigung {
    pub erforderlich: bool,
    pub form: &'static str,
    pub rechtsgrundlage: &'static str,
    #[schemars(extend("format" = "date"))]
    pub vertragsende: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Gekuendigt {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub rahmenvertrag_id: Uuid,
    pub rahmenvertrag_nr: String,
    #[schemars(extend("format" = "date"))]
    pub lieferende: String,
    pub grund: Kuendigungsgrund,
    pub rechtsgrundlage: &'static str,
    pub kuendigungsbestaetigung: Bestaetigung,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Kuendigung {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    #[schemars(extend("format" = "date"))]
    pub lieferende: String,
    #[schemars(extend("format" = "date"))]
    pub eingang: String,
    pub grund: Kuendigungsgrund,
    pub frist: String,
    pub rechtsgrundlage: &'static str,
    pub mako_dispatched: usize,
    pub kuendigungsbestaetigung: Bestaetigung,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Tarifwechsel {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub komp_id: Uuid,
    pub malo_id: Option<String>,
    pub old_product_code: Option<String>,
    pub new_product_code: String,
    #[schemars(extend("format" = "date"))]
    pub wirksamkeit: String,
    pub geplant: bool,
    pub initiator: Initiator,
    pub preisanpassungsanzeige_erforderlich: bool,
    pub sonderkuendigungsrecht: Sonderkuendigungsrecht,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PreisgarantieHinterlegt {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    #[schemars(extend("format" = "date"))]
    pub preisgarantie_bis: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PreisaenderungAnkuendigung {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub komp_id: Uuid,
    pub malo_id: Option<String>,
    pub sparte: String,
    pub current_product_code: Option<String>,
    pub new_product_code: String,
    #[schemars(extend("format" = "date"))]
    pub wirksamkeit: String,
    pub vorlauf_tage: i64,
    pub erforderliche_frist: String,
    #[schemars(extend("format" = "date"))]
    pub fruehestens_wirksam: String,
    pub frist_gewahrt: bool,
    pub rechtsgrundlage: &'static str,
    #[schemars(with = "Option<String>", extend("format" = "uuid"))]
    pub dokument_id: Option<Uuid>,
    pub anlass: Option<String>,
    pub umfang: Vec<PreisPosition>,
    pub umfang_vollstaendig: bool,
    pub sonderkuendigungsrecht: Sonderkuendigungsrecht,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AutoerneuerungAnkuendigung {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    #[schemars(extend("format" = "date"))]
    pub vertragsende: String,
    pub verlaengerung: Verlaengerungsart,
    pub kuendigungsfrist_monate: i32,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Abgeschlossen {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    #[schemars(extend("const" = "ABGELAUFEN"))]
    pub status: &'static str,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AblaufAnkuendigung {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    pub vertragsart: String,
    #[schemars(extend("format" = "date"))]
    pub vertragsende: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub preisgarantie_bis: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub faellig_am: String,
    pub auto_renewal: bool,
    pub kundentyp: String,
    pub standort_bezeichnung: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Bestellnachweis {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub id: Uuid,
    #[schemars(regex(pattern = "^[a-f0-9]{64}$"))]
    pub sha256: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BestellungEingegangen {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub bestellung_id: Uuid,
    pub bestellreferenz: String,
    pub bestellnachweis: Bestellnachweis,
    #[schemars(extend("format" = "date-time"))]
    pub eingegangen_am: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BestellungAngenommen {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub bestellung_id: Uuid,
    pub bestellreferenz: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    pub vertrags_nr: String,
    #[schemars(extend("format" = "date-time"))]
    pub vertragsschluss_am: String,
    pub widerrufsfrist: Option<Widerrufsfrist>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct BestellungAbgelehnt {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub bestellung_id: Uuid,
    pub bestellreferenz: String,
    pub grund: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Widerrufsfrist {
    #[schemars(extend("format" = "date"))]
    pub beginn: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub ende: String,
    #[schemars(extend("format" = "date"))]
    pub erloeschen: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct WiderrufEingegangen {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub bestellung_id: Uuid,
    pub bestellreferenz: String,
    #[schemars(with = "Option<String>", extend("format" = "uuid"))]
    pub vertrag_id: Option<Uuid>,
    pub widerruf: Widerruf,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct Widerruf {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub widerruf_id: Uuid,
    pub rechtzeitig: bool,
    pub frist: WiderrufTermine,
    pub wirkung: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub lieferende: Option<String>,
    pub wertersatz_geschuldet: Option<bool>,
    #[schemars(extend("format" = "date-time"))]
    pub eingegangen_am: String,
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub eingangsbestaetigung: Uuid,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct WiderrufTermine {
    #[schemars(extend("format" = "date"))]
    pub beginn: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub ende: Option<String>,
    #[schemars(extend("format" = "date"))]
    pub erloeschen: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct TarifwechselGeplant(pub Tarifwechsel);

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Sonderkuendigungsrecht {
    Gewaehrt {
        #[schemars(extend("const" = true))]
        besteht: bool,
        rechtsgrundlage: &'static str,
        #[schemars(extend("format" = "date"))]
        wirksam_zum: String,
        entgeltfrei: bool,
    },
    Keines {
        #[schemars(extend("const" = false))]
        besteht: bool,
    },
}

impl Sonderkuendigungsrecht {
    #[must_use]
    pub fn zum(wirksamkeit: time::Date) -> Self {
        Self::Gewaehrt {
            besteht: true,
            rechtsgrundlage: "§ 41 Abs. 5 Satz 4 EnWG",
            wirksam_zum: wirksamkeit.to_string(),
            entgeltfrei: true,
        }
    }
}

impl Bestaetigung {
    #[must_use]
    pub fn zum(lieferende: time::Date) -> Self {
        Self {
            erforderlich: true,
            form: "Textform",
            rechtsgrundlage: "§ 41 Abs. 8 Nr. 2 EnWG",
            vertragsende: lieferende.to_string(),
        }
    }
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "art", rename_all = "UPPERCASE")]
pub enum Verlaengerungsart {
    Unbefristet {
        kuendigungsfrist: &'static str,
        rechtsgrundlage: &'static str,
    },
    Befristet {
        #[schemars(extend("format" = "date"))]
        neues_vertragsende: String,
        monate: i32,
    },
}

impl mako_service::EventPayload for Aktiv {
    const TYPE: &'static str = mako_events::vertrag::AKTIV;
}

impl mako_service::EventPayload for KuendigungWiderrufen {
    const TYPE: &'static str = mako_events::vertrag::KUENDIGUNG_WIDERRUFEN;
}

impl mako_service::EventPayload for Gekuendigt {
    const TYPE: &'static str = mako_events::vertrag::GEKUENDIGT;
}

impl mako_service::EventPayload for Kuendigung {
    const TYPE: &'static str = mako_events::vertrag::KUENDIGUNG;
}

impl mako_service::EventPayload for Tarifwechsel {
    const TYPE: &'static str = mako_events::vertrag::TARIFWECHSEL;
}

impl mako_service::EventPayload for PreisgarantieHinterlegt {
    const TYPE: &'static str = mako_events::vertrag::PREISGARANTIE_HINTERLEGT;
}

impl mako_service::EventPayload for PreisaenderungAnkuendigung {
    const TYPE: &'static str = mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG;
}

impl mako_service::EventPayload for AutoerneuerungAnkuendigung {
    const TYPE: &'static str = mako_events::vertrag::AUTOERNEUERUNG_ANKUENDIGUNG;
}

impl mako_service::EventPayload for Abgeschlossen {
    const TYPE: &'static str = mako_events::vertrag::ABGESCHLOSSEN;
}

impl mako_service::EventPayload for AblaufAnkuendigung {
    const TYPE: &'static str = mako_events::vertrag::ABLAUF_ANKUENDIGUNG;
}

impl mako_service::EventPayload for BestellungEingegangen {
    const TYPE: &'static str = mako_events::vertrag::BESTELLUNG_EINGEGANGEN;
}

impl mako_service::EventPayload for BestellungAngenommen {
    const TYPE: &'static str = mako_events::vertrag::BESTELLUNG_ANGENOMMEN;
}

impl mako_service::EventPayload for BestellungAbgelehnt {
    const TYPE: &'static str = mako_events::vertrag::BESTELLUNG_ABGELEHNT;
}

impl mako_service::EventPayload for WiderrufEingegangen {
    const TYPE: &'static str = mako_events::vertrag::WIDERRUF_EINGEGANGEN;
}

impl mako_service::EventPayload for TarifwechselGeplant {
    const TYPE: &'static str = mako_events::vertrag::TARIFWECHSEL_GEPLANT;
}
