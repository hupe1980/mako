//! The MSB's answers to the ORDERS it receives, and the doors that send the
//! ORDERS a requesting side owes.
//!
//! | Command | Workflow | Message |
//! |---|---|---|
//! | `gpke.anfrage-sperrung.beantworten` | `gpke-sperrung-msb` | ORDRSP 19118 / 19119 out of `E_0488` |
//! | `geli.anfrage-sperrung.beantworten` | `geli-gas-sperrung-msb` | ORDRSP 19118 / 19119 out of `E_1001` |
//! | `wim.technik-aenderung.beantworten` | `wim-technik-aenderung` | ORDRSP 19005 / 19006 out of `E_0249` / `E_0250` / `E_0279` / `E_0283` |
//!
//! Each answer is rendered and held to its AHB profile before anything is
//! written (`eog_stammdaten::refuse_what_the_gate_refuses`) — the check the
//! pre-send gate runs on the outbox entry — so a refused answer leaves the
//! process open and its window running instead of dead-lettering at the gate.

use super::*;

/// Body of `gpke.anfrage-sperrung.beantworten` and
/// `geli.anfrage-sperrung.beantworten`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnfrageSperrungAntwortBody {
    /// The Marktlokation the Anfrage names — its `SG2 LOC+172`.
    malo_id: String,
    /// The code of `E_0488` (Strom) resp. `E_1001` (Gas); its Cluster picks
    /// 19118 or 19119.
    antwortcode: String,
    /// `SG2 FTX+AAP` — the problem named; Muss beside `A02`/`A03`.
    #[serde(default)]
    freitext: Option<String>,
}

fn parse_body<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<T, DispatchError> {
    serde_json::from_value(payload.clone())
        .map_err(|e| DispatchError::InvalidPayload(format!("payload: {e}")))
}

fn required<'a>(raw: &'a str, field: &str) -> Result<&'a str, DispatchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DispatchError::InvalidPayload(format!(
            "{field:?} must not be empty"
        )));
    }
    Ok(trimmed)
}

// ── Anfrage Sperrung (ORDERS 17116 → ORDRSP 19118 / 19119) ────────────────────

pub(super) fn cmd_gpke_anfrage_sperrung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_anfrage_sperrung_antwort(s, p))
}

/// The MSB Strom answers the NB's Anfrage Sperrung out of `E_0488` (GPKE Teil
/// 2 Kap. 3.5.1.2 Nr. 4, S. 107; ORDRSP AHB 1.1b Kap. 4.3.1, S. 17–18).
async fn dispatch_gpke_anfrage_sperrung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::sperrung_msb::{GpkeSperrungMsbWorkflow, SperrungMsbCommand, WORKFLOW_NAME};
    let body: AnfrageSperrungAntwortBody = parse_body(payload)?;
    let malo = required(&body.malo_id, "malo_id")?.to_owned();
    let cmd = SperrungMsbCommand::SendAntwort {
        antwortcode: required(&body.antwortcode, "antwortcode")?.to_owned(),
        freitext: body.freitext,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
    };
    dispatch_to_process_in_version::<GpkeSperrungMsbWorkflow, _>(
        state,
        &malo,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GpkeSperrungMsbWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

pub(super) fn cmd_geli_anfrage_sperrung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_geli_anfrage_sperrung_antwort(s, p))
}

/// The gMSB answers the GNB's Anfrage Sperrung out of `E_1001` (EBD 4.3 Kap.
/// 18.2.2; ORDRSP AHB 1.1b Kap. 4.3.1, S. 17–18).
async fn dispatch_geli_anfrage_sperrung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_geli_gas::sperrung_msb::{
        GeliGasSperrungMsbWorkflow, SperrungMsbGasCommand, WORKFLOW_NAME,
    };
    let body: AnfrageSperrungAntwortBody = parse_body(payload)?;
    let malo = required(&body.malo_id, "malo_id")?.to_owned();
    let cmd = SperrungMsbGasCommand::SendAntwort {
        antwortcode: required(&body.antwortcode, "antwortcode")?.to_owned(),
        freitext: body.freitext,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
    };
    dispatch_to_process_in_version::<GeliGasSperrungMsbWorkflow, _>(
        state,
        &malo,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GeliGasSperrungMsbWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

// ── Änderung der Technik (ORDERS 17011 → ORDRSP 19005 / 19006) ───────────────

/// Body of `wim.technik-aenderung.beantworten`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TechnikAntwortBody {
    /// MP-ID of the NB or LF that sent the Auftrag — its `NAD+MS`.
    absender_mp_id: String,
    /// The Auftrag's `BGM` DE 1004.
    dokumentennummer: String,
    /// `NB` or `LF` — with the Auftrag's `BGM` DE 1001 it selects `E_0249`,
    /// `E_0250`, `E_0279` or `E_0283`.
    besteller: String,
    /// The code of that tree; its Cluster picks 19005 or 19006.
    antwortcode: String,
    /// `SG2 FTX+AAP` — Muss beside `A99`.
    #[serde(default)]
    freitext: Option<String>,
    /// `SG6 CTA`/`COM` — Muss on both answers.
    ansprechpartner: mako_wim::geraeteubernahme::Ansprechpartner,
}

pub(super) fn cmd_wim_technik_aenderung_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_technik_aenderung_antwort(s, p))
}

/// The MSB answers a received ORDERS 17011 (WiM Teil 1 Kap. 3.3.1.2 / 3.3.2.2
/// Nr. 2, S. 53; ORDRSP AHB 1.1b Kap. 4.12, S. 50–53).
async fn dispatch_wim_technik_aenderung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_pruefung::codes::TechnikBesteller;
    use mako_wim::technik_aenderung::{
        TechnikAenderungCommand, WORKFLOW_NAME, WimTechnikAenderungWorkflow, auftrag_key,
    };
    let body: TechnikAntwortBody = parse_body(payload)?;
    let key = auftrag_key(
        required(&body.absender_mp_id, "absender_mp_id")?,
        required(&body.dokumentennummer, "dokumentennummer")?,
    );
    let besteller = match body.besteller.trim().to_uppercase().as_str() {
        "NB" => TechnikBesteller::Netzbetreiber,
        "LF" => TechnikBesteller::Lieferant,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "\"besteller\" must be NB or LF; got {other:?}"
            )));
        }
    };
    let cmd = TechnikAenderungCommand::SendAntwort {
        besteller,
        antwortcode: required(&body.antwortcode, "antwortcode")?.to_owned(),
        freitext: body.freitext,
        ansprechpartner: body.ansprechpartner,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
    };
    dispatch_to_process_in_version::<WimTechnikAenderungWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = WimTechnikAenderungWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

// ── Datenabruf (ORDERS 17102 / 17004 / 17113 → ORDRSP 19102 / 19007 / 19114) ──

/// Body of `gpke.datenabruf.ablehnen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatenabrufAblehnungBody {
    /// MP-ID of the Anfragender — the ORDERS' `NAD+MS`.
    anfragender_mp_id: String,
    /// The ORDERS' `BGM` DE 1004.
    dokumentennummer: String,
    /// The tree the code is read from (`E_0442`, `E_0219`–`E_0222`, `E_0225`,
    /// `E_0226`–`E_0230`), as the ORDERS' PID admits it.
    codeliste: String,
    /// The code.
    antwortcode: String,
    /// `SG6 CTA`/`COM` — Muss on a 19114.
    #[serde(default)]
    ansprechpartner: Option<mako_wim::geraeteubernahme::Ansprechpartner>,
}

pub(super) fn cmd_gpke_datenabruf_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_datenabruf_ablehnung(s, p))
}

/// The MSB refuses an Anfrage, Anforderung or Reklamation von Werten on its
/// ORDRSP (ORDRSP AHB 1.1b Kap. 4.1.3, 4.13, 4.8).
async fn dispatch_gpke_datenabruf_ablehnung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::datenabruf_msb::{
        DatenabrufMsbCommand, GpkeDatenabrufMsbWorkflow, WORKFLOW_NAME, anfrage_key,
    };
    let body: DatenabrufAblehnungBody = parse_body(payload)?;
    let key = anfrage_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        required(&body.dokumentennummer, "dokumentennummer")?,
    );
    let ansprechpartner = body
        .ansprechpartner
        .map(|a| serde_json::to_value(a).map_err(|e| DispatchError::InvalidPayload(e.to_string())))
        .transpose()?;
    let cmd = DatenabrufMsbCommand::SendAblehnung {
        codeliste: required(&body.codeliste, "codeliste")?.to_owned(),
        antwortcode: required(&body.antwortcode, "antwortcode")?.to_owned(),
        ansprechpartner,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
    };
    dispatch_to_process_in_version::<GpkeDatenabrufMsbWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GpkeDatenabrufMsbWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

/// Body of `gpke.datenabruf.werte-senden`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatenabrufWerteBody {
    /// MP-ID of the Anfragender — the ORDERS' `NAD+MS`.
    anfragender_mp_id: String,
    /// The ORDERS' `BGM` DE 1004.
    dokumentennummer: String,
    /// A Lastgang Anfrage's interval values, each with `quantity_kwh`,
    /// `dtm_from`, `dtm_to` (`CCYYMMDDHHMM+00`), `obis_code` and, for a
    /// substitute value, `ersatzwert`.
    #[serde(default)]
    werte: Vec<mako_engine::payload::MsconsRead>,
    /// A Zählerstand Anfrage's Stände (MSCONS 13017), each with `obis_code`,
    /// `quantity`, `gueltig` (the day), optionally `ablesedatum`, and for a
    /// substitute value `ersatzwert` with `ersatzwert_verfahren` and
    /// `ersatzwert_grund`.
    #[serde(default)]
    zaehlerstaende: Vec<mako_engine::payload::MsconsZaehlerstand>,
    /// 13017: the Zähler (`SG7 RFF+MG`).
    #[serde(default)]
    geraetenummer: Option<String>,
    /// 13017: the iMS Konfigurations-ID (`SG7 RFF+AGK`), instead of the Zähler.
    #[serde(default)]
    konfigurations_id: Option<String>,
}

pub(super) fn cmd_gpke_datenabruf_werte_senden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_datenabruf_werte(s, p))
}

/// The MSB answers an Anfrage von Werten 17102 with the Lastgang it asked
/// for, on the MSCONS `datenabruf_msb::werte_pid` names, or with the
/// Zählerstände on MSCONS 13017; the rendered MSCONS passes the gate before
/// anything is written.
async fn dispatch_gpke_datenabruf_werte(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::datenabruf_msb::{
        DatenabrufMsbCommand, GpkeDatenabrufMsbWorkflow, WORKFLOW_NAME, anfrage_key,
    };
    let body: DatenabrufWerteBody = parse_body(payload)?;
    let key = anfrage_key(
        required(&body.anfragender_mp_id, "anfragender_mp_id")?,
        required(&body.dokumentennummer, "dokumentennummer")?,
    );
    let message_ref = MessageRef::new(uuid::Uuid::new_v4().simple().to_string());
    let cmd = if body.zaehlerstaende.is_empty() {
        if body.geraetenummer.is_some() || body.konfigurations_id.is_some() {
            return Err(DispatchError::InvalidPayload(
                "\"geraetenummer\" / \"konfigurations_id\" belong to \"zaehlerstaende\"".into(),
            ));
        }
        DatenabrufMsbCommand::SendWerte {
            werte: body.werte,
            message_ref,
        }
    } else {
        if !body.werte.is_empty() {
            return Err(DispatchError::InvalidPayload(
                "a body carries \"werte\" (Lastgang) or \"zaehlerstaende\" (MSCONS 13017), \
                 not both"
                    .into(),
            ));
        }
        DatenabrufMsbCommand::SendZaehlerstaende {
            geraetenummer: body.geraetenummer,
            konfigurations_id: body.konfigurations_id,
            staende: body.zaehlerstaende,
            message_ref,
        }
    };
    dispatch_to_process_in_version::<GpkeDatenabrufMsbWorkflow, _>(
        state,
        &key,
        WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GpkeDatenabrufMsbWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

// ── GPKE Teil 3 Konfiguration — the recipient's ORDRSP ───────────────────────

/// Body of `gpke.konfiguration.beantworten`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KonfigurationAntwortBody {
    /// MP-ID of the ORDERS' sender — its `NAD+MS`.
    absender_mp_id: String,
    /// The ORDERS' `BGM` DE 1004.
    dokumentennummer: String,
    /// The tree the answer's column admits for the party answered.
    codeliste: String,
    /// The code.
    antwortcode: String,
    /// `SG2 FTX+AAP` — Muss where the code needs the problem named.
    #[serde(default)]
    freitext: Option<String>,
}

pub(super) fn cmd_gpke_konfiguration_beantworten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_konfiguration_antwort(s, p))
}

/// The recipient answers a received GPKE Teil 3 ORDERS on the ORDRSP its
/// column pairs with it — `mako_gpke::konfiguration_aenderung::antwort_auf`
/// (ORDRSP AHB 1.1b Kap. 4.4–4.7).
async fn dispatch_gpke_konfiguration_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::konfiguration_aenderung::auftrag_key;
    use mako_gpke::{
        GpkeKonfigurationAenderungWorkflow, KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
        KonfigurationAenderungCommand,
    };
    let body: KonfigurationAntwortBody = parse_body(payload)?;
    let key = auftrag_key(
        required(&body.absender_mp_id, "absender_mp_id")?,
        required(&body.dokumentennummer, "dokumentennummer")?,
    );
    let cmd = KonfigurationAenderungCommand::SendAntwort {
        codeliste: required(&body.codeliste, "codeliste")?.to_owned(),
        antwortcode: required(&body.antwortcode, "antwortcode")?.to_owned(),
        freitext: body.freitext,
        message_ref: MessageRef::new(uuid::Uuid::new_v4().simple().to_string()),
    };
    dispatch_to_process_in_version::<GpkeKonfigurationAenderungWorkflow, _>(
        state,
        &key,
        KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
        move |current, fv| {
            use mako_engine::workflow::Workflow as _;
            let out = GpkeKonfigurationAenderungWorkflow::handle(current, cmd.clone())
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
            super::eog_stammdaten::refuse_what_the_gate_refuses(state, fv, &out.outbox)?;
            Ok(cmd)
        },
    )
    .await
}

// ── Requesting side: the Anforderung von Werten (ORDERS 17004) ───────────────

/// Body of `gpke.datenabruf.anfordern`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatenabrufAnforderungBody {
    /// MP-ID of the MSB asked.
    msb_mp_id: String,
    /// The Markt- or Messlokation — `SG2 LOC+172` (the Marktlokation beside
    /// `Z46`, the Messlokation otherwise; ORDERS AHB 1.1b Kap. 4.13).
    lokation_id: String,
    /// `SG29 IMD` DE 7081 Grund der Anforderung (`Z46`, `Z47`, `Z49`).
    grund: String,
    /// `SG29 DTM+7` — the day the values are asked for (`YYYY-MM-DD`).
    wertzeitpunkt: String,
    /// `BGM` DE 1004 (`an..35`), which the MSB's Ablehnung echoes in `SG1
    /// RFF+ON`; generated when absent.
    #[serde(default)]
    dokumentennummer: Option<String>,
}

pub(super) fn cmd_gpke_datenabruf_anfordern<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_datenabruf_anforderung(s, p))
}

/// The LF, NB or MSB sends an Anforderung von Werten 17004 to the MSB and
/// opens the process the MSB's ORDRSP 19007 resumes by its `RFF+ON`.
async fn dispatch_gpke_datenabruf_anforderung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_gpke::{DATENABRUF_WORKFLOW_NAME, DatenabrufCommand, GpkeDatenabrufWorkflow};
    let body: DatenabrufAnforderungBody = parse_body(payload)?;
    let nummer = match body.dokumentennummer.as_deref() {
        Some(raw) => {
            let n = required(raw, "dokumentennummer")?;
            if n.chars().count() > 35 {
                return Err(DispatchError::InvalidPayload(format!(
                    "\"dokumentennummer\" {n:?} exceeds BGM DE 1004's an..35"
                )));
            }
            n.to_owned()
        }
        None => format!("WA{}", uuid::Uuid::new_v4().simple()),
    };
    let grund = required(&body.grund, "grund")?;
    if !matches!(grund, "Z46" | "Z47" | "Z49") {
        return Err(DispatchError::InvalidPayload(format!(
            "\"grund\" must be Z46, Z47 or Z49 (Strom, ORDERS AHB 1.1b Kap. 4.13); got {grund:?}"
        )));
    }
    let command = DatenabrufCommand::SendAnfrage {
        orders_pid: Pruefidentifikator::const_new(17_004),
        recipient: MarktpartnerCode::new(required(&body.msb_mp_id, "msb_mp_id")?),
        malo: Some(MaLo::new(required(&body.lokation_id, "lokation_id")?)),
        message_ref: MessageRef::new(nummer.clone()),
        payload: serde_json::json!({
            "sender": state.sender_party_id,
            "grund": grund,
            "wertzeitpunkt": required(&body.wertzeitpunkt, "wertzeitpunkt")?,
        }),
        gesendet_am: time::OffsetDateTime::now_utc(),
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = GpkeDatenabrufWorkflow::handle(&Default::default(), command.clone())
            .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(
            state,
            &latest_format_version(),
            &out.outbox,
        )?;
    }
    let process_id = spawn_process::<GpkeDatenabrufWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(DATENABRUF_WORKFLOW_NAME, latest_format_version()),
        command,
        &[],
        &[nummer.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

// ── Requesting side: the Änderung der Technik (ORDERS 17011 `Z12`) ───────────

/// Body of `wim.technik-aenderung.bestellen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TechnikAenderungBestellungBody {
    /// `NB` or `LF` — the Marktrolle ordering, which bounds the Produkte.
    besteller: String,
    /// MP-ID of the MSB of the Messlokation.
    msb_mp_id: String,
    /// The Messlokation — `SG2 LOC+172` (`[522]`).
    melo_id: String,
    /// `DTM+203` — the day the change is wanted (`YYYY-MM-DD`).
    ausfuehrungsdatum: String,
    /// The Produkt-Codes, one `SG29 LIN+Z19` each (Codeliste der
    /// Konfigurationen Kap. 7, Ebene Messlokation).
    produkte: Vec<String>,
    /// `SG3 RFF+Z18` — the Marktlokation, with „weitere Energieflussrichtung".
    #[serde(default)]
    marktlokation: Option<String>,
    /// `SG2 NAD+DEB` — the MSB der Marktlokation, with „weitere
    /// Energieflussrichtung".
    #[serde(default)]
    msb_marktlokation: Option<String>,
    /// `BGM` DE 1004 (`an..35`), which the MSB's ORDRSP echoes in `SG1
    /// RFF+ON`; generated when absent.
    #[serde(default)]
    dokumentennummer: Option<String>,
}

pub(super) fn cmd_wim_technik_aenderung_bestellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_wim_technik_aenderung_bestellung(s, p))
}

/// The NB or LF orders an Änderung der Technik of a Messlokation (ORDERS
/// 17011 `BGM+Z12`, ORDERS AHB 1.1b Kap. 4.12) and opens the process the
/// MSB's ORDRSP 19005 / 19006 resumes by its `RFF+ON`.
///
/// A Lieferant may order only the Produkte the Codeliste der Konfigurationen
/// 1.4 Kap. 7 (S. 58) marks LF-bestellbar (Bedingung `[157]`); a code off the
/// Messlokation list is refused (`[153]`).
async fn dispatch_wim_technik_aenderung_bestellung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_wim::WimTechnikAenderungWorkflow;
    use mako_wim::technik_aenderung::{
        MESSLOKATION_PRODUKTE, TechnikAenderungCommand, WORKFLOW_NAME,
    };
    let body: TechnikAenderungBestellungBody = parse_body(payload)?;
    let nummer = match body.dokumentennummer.as_deref() {
        Some(raw) => {
            let n = required(raw, "dokumentennummer")?;
            if n.chars().count() > 35 {
                return Err(DispatchError::InvalidPayload(format!(
                    "\"dokumentennummer\" {n:?} exceeds BGM DE 1004's an..35"
                )));
            }
            n.to_owned()
        }
        None => format!("TA{}", uuid::Uuid::new_v4().simple()),
    };
    if body.produkte.is_empty() {
        return Err(DispatchError::InvalidPayload(
            "\"produkte\" names no Produkt — the 17011 carries one SG29 per Produkt".to_owned(),
        ));
    }
    let als_lf = match body.besteller.as_str() {
        "LF" => true,
        "NB" => false,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "\"besteller\" must be NB or LF; got {other:?}"
            )));
        }
    };
    if !state
        .configured_marktrollen
        .iter()
        .any(|r| r == &body.besteller)
    {
        return Err(DispatchError::InvalidPayload(format!(
            "this deployment does not play the Besteller role {}",
            body.besteller
        )));
    }
    for code in &body.produkte {
        let Some((_, bezeichnung, lf_bestellbar)) =
            MESSLOKATION_PRODUKTE.iter().find(|(c, _, _)| c == code)
        else {
            return Err(DispatchError::InvalidPayload(format!(
                "Produkt {code:?} is not a Messlokation product of Codeliste der \
                 Konfigurationen Kap. 7"
            )));
        };
        if als_lf && !lf_bestellbar {
            return Err(DispatchError::InvalidPayload(format!(
                "a Lieferant cannot order {bezeichnung:?} ({code}) from the MSB"
            )));
        }
    }
    let command = TechnikAenderungCommand::SendAuftrag {
        orders_pid: Pruefidentifikator::const_new(17_011),
        recipient: MarktpartnerCode::new(required(&body.msb_mp_id, "msb_mp_id")?),
        location_id: Some(required(&body.melo_id, "melo_id")?.to_owned()),
        message_ref: MessageRef::new(nummer.clone()),
        payload: serde_json::to_value(mako_engine::payload::OrdersPayload {
            sender: Some(state.sender_party_id.clone()),
            sparte: Some(mako_engine::types::Sparte::Strom),
            ausfuehrungsdatum: Some(
                required(&body.ausfuehrungsdatum, "ausfuehrungsdatum")?.to_owned(),
            ),
            produkte: body.produkte,
            marktlokation: body.marktlokation,
            msb_marktlokation: body.msb_marktlokation,
            ..Default::default()
        })
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?,
        gesendet_am: time::OffsetDateTime::now_utc(),
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = WimTechnikAenderungWorkflow::handle(&Default::default(), command.clone())
            .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(
            state,
            &latest_format_version(),
            &out.outbox,
        )?;
    }
    let process_id = spawn_process::<WimTechnikAenderungWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(WORKFLOW_NAME, latest_format_version()),
        command,
        &[],
        &[nummer.as_str()],
    )
    .await?;
    Ok(DispatchOutcome::Spawned { process_id })
}

// ── MSBN side: Gerätewechselabsicht 17009 and Bestellung 17001 ───────────────

/// One device of a Gerätewechselabsicht: `SG30 CCI` and `SG34 RFF+Z09`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GeraetBody {
    /// `CCI` DE 7037 Geräteart (`Z25` Wandler, `ZD4` Zähleinrichtung, …).
    geraeteart: String,
    /// The Gerätenummern — up to three for a Wandler, one otherwise.
    geraetenummern: Vec<String>,
}

/// Body of `wim.geraetewechselabsicht.anzeigen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbsichtBody {
    /// MP-ID of the MSBA.
    msba_mp_id: String,
    /// The Messlokation — `SG2 LOC+172`.
    melo_id: String,
    /// `DTM+203` Gerätewechseltermin (`YYYY-MM-DD`).
    geraetewechseltermin: String,
    /// `IMD` DE 7081: `Z58` Wechsel sämtlicher, `Z59` teilweiser Wechsel.
    umfang: String,
    /// The devices to be changed, one `SG29` each.
    geraete: Vec<GeraetBody>,
    /// `strom` or `gas` — the Sparte the Termin is a day boundary of.
    sparte: String,
    /// `BGM` DE 1004 (`an..35`); generated when absent.
    #[serde(default)]
    dokumentennummer: Option<String>,
}

/// Body of `wim.geraeteuebernahme.bestellen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BestellungBody {
    /// MP-ID of the MSBA.
    msba_mp_id: String,
    /// `SG1 RFF+AAG` — the QUOTES' `BGM` DE 1004.
    angebotsnummer: String,
    /// `DTM+203` Ausführungsdatum (`YYYY-MM-DD`).
    ausfuehrungsdatum: String,
    /// `SG5 CTA`/`COM` — the technischer Ansprechpartner.
    ansprechpartner: mako_wim::geraeteubernahme::Ansprechpartner,
    /// `SG34 RFF+Z03` — the QUOTES' `LIN` positions ordered.
    positionen: Vec<String>,
    /// `strom` or `gas`.
    sparte: String,
    /// `BGM` DE 1004 (`an..35`); generated when absent.
    #[serde(default)]
    dokumentennummer: Option<String>,
}

fn dokumentennummer(raw: Option<&str>, prefix: &str) -> Result<String, DispatchError> {
    match raw {
        Some(raw) => {
            let n = required(raw, "dokumentennummer")?;
            if n.chars().count() > 35 {
                return Err(DispatchError::InvalidPayload(format!(
                    "\"dokumentennummer\" {n:?} exceeds BGM DE 1004's an..35"
                )));
            }
            Ok(n.to_owned())
        }
        None => Ok(format!("{prefix}{}", uuid::Uuid::new_v4().simple())),
    }
}

fn sparte(raw: &str) -> Result<&'static str, DispatchError> {
    match raw.trim().to_lowercase().as_str() {
        "strom" => Ok("strom"),
        "gas" => Ok("gas"),
        other => Err(DispatchError::InvalidPayload(format!(
            "\"sparte\" must be strom or gas; got {other:?}"
        ))),
    }
}

pub(super) fn cmd_wim_geraetewechselabsicht_anzeigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let body: AbsichtBody = parse_body(p)?;
        if !matches!(body.umfang.as_str(), "Z58" | "Z59") {
            return Err(DispatchError::InvalidPayload(format!(
                "\"umfang\" must be Z58 or Z59 (ORDERS AHB 1.1b Kap. 4.10); got {:?}",
                body.umfang
            )));
        }
        let nummer = dokumentennummer(body.dokumentennummer.as_deref(), "GWA")?;
        let payload = serde_json::json!({
            "location": required(&body.melo_id, "melo_id")?,
            "ausfuehrungsdatum": required(&body.geraetewechseltermin, "geraetewechseltermin")?,
            "sparte": sparte(&body.sparte)?,
            "umfang": body.umfang,
            "geraete": body.geraete,
        });
        send_msbn_auftrag(
            s,
            mako_wim::geraeteubernahme_msbn::ABSICHT_PID,
            &body.msba_mp_id,
            nummer,
            payload,
        )
        .await
    })
}

pub(super) fn cmd_wim_geraeteuebernahme_bestellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let body: BestellungBody = parse_body(p)?;
        let nummer = dokumentennummer(body.dokumentennummer.as_deref(), "GUE")?;
        let payload = serde_json::json!({
            "angebotsnummer": required(&body.angebotsnummer, "angebotsnummer")?,
            "ausfuehrungsdatum": required(&body.ausfuehrungsdatum, "ausfuehrungsdatum")?,
            "sparte": sparte(&body.sparte)?,
            "ansprechpartner": body.ansprechpartner,
            "positionen": body.positionen,
        });
        send_msbn_auftrag(
            s,
            mako_wim::geraeteubernahme_msbn::BESTELLUNG_PID,
            &body.msba_mp_id,
            nummer,
            payload,
        )
        .await
    })
}

/// The MSBN sends its ORDERS and opens the process the MSBA's ORDRSP resumes
/// by `SG1 RFF+ON`; the rendered ORDERS passes the gate before anything is
/// written.
async fn send_msbn_auftrag(
    state: &CommandsApiState,
    pid: u32,
    msba: &str,
    nummer: String,
    payload: serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_wim::geraeteubernahme_msbn::{
        MsbnAuftrag, MsbnCommand, WORKFLOW_NAME, WimGeraeteubernahmeMsbnWorkflow,
    };
    let command = MsbnCommand::SendAuftrag {
        auftrag: MsbnAuftrag {
            pid: Pruefidentifikator::new(pid)
                .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?,
            msbn: MarktpartnerCode::new(state.sender_party_id.clone()),
            msba: MarktpartnerCode::new(required(msba, "msba_mp_id")?),
            dokumentennummer: nummer.clone(),
        },
        payload,
    };
    {
        use mako_engine::workflow::Workflow as _;
        let out = WimGeraeteubernahmeMsbnWorkflow::handle(&Default::default(), command.clone())
            .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
        super::eog_stammdaten::refuse_what_the_gate_refuses(
            state,
            &latest_format_version(),
            &out.outbox,
        )?;
    }
    let occupancy =
        find_occupying_process::<WimGeraeteubernahmeMsbnWorkflow>(state, &nummer, WORKFLOW_NAME)
            .await?;
    if let Some(process_id) = occupancy.process {
        return Err(DispatchError::DuplicateProcess {
            process_id,
            malo_id: nummer,
        });
    }
    let process_id = spawn_process::<WimGeraeteubernahmeMsbnWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(WORKFLOW_NAME, latest_format_version()),
        command,
        &[],
        &[nummer.as_str()],
    )
    .await?;
    drop(occupancy);
    Ok(DispatchOutcome::Spawned { process_id })
}
