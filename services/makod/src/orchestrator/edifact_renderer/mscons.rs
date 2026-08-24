//! MSCONS renderers (Summenzeitreihe, Energiemenge, Typ-2 Werte).
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
/// MSCONS "Übertragung Summenzeitreihe" (MaBiS), AHB 3.2 §8.3.1.
pub(super) const MSCONS_PID_SUMMENZEITREIHE: u64 = 13003;

/// BGM DE 1001 document-name code for an MSCONS Anwendungsfall.
///
/// The code is not constant across MSCONS: it names what kind of document the
/// message is, and the AHB fixes a different one per use case. Sending the
/// wrong code labels a Summenzeitreihe as a Prozessdatenbericht, which the
/// receiver routes by.
pub(super) const fn mscons_document_code(pid: u64) -> &'static str {
    match pid {
        // "Zeitreihen im Rahmen der Bilanzkreisabrechnung"
        MSCONS_PID_SUMMENZEITREIHE => "BK",
        // "Redispatch"
        MSCONS_PID_AUSFALLARBEIT_SZR => "Z46",
        // "Bewegungsdaten im Kalenderjahr vor Lieferbeginn"
        MSCONS_PID_ARBEIT_LEISTUNGSMAX => "Z27",
        // "Energiemenge und Leistungsmaximum"
        MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX => "Z28",
        // "Werte nach Typ 2" (MSCONS AHB 3.2 §11.2). A `7`
        // („Prozessdatenbericht") here is refused by the generated rule
        // `AHB-13027-BGM-1001-Q`, which admits only `Z83`.
        MSCONS_PID_WERTE_TYP2 => "Z83",
        // "Prozessdatenbericht"
        _ => "7",
    }
}

/// MSCONS "Energiemenge (Strom)", AHB 3.2 — energy for a billing period, with
/// no power maximum.
pub(super) const MSCONS_PID_ENERGIEMENGE: u64 = 13019;

/// MSCONS "Energiemenge und Leistungsmaximum", AHB 3.2.
pub(super) const MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX: u64 = 13016;

/// MSCONS "Arbeit / Leistungsmaximum im Kalenderjahr vor Lieferbeginn",
/// AHB 3.2 — the movement data a Netznutzungsvertrag requires when an RLM
/// Marktlokation changes supplier mid-year (GPKE Kap. 6.1).
pub(super) const MSCONS_PID_ARBEIT_LEISTUNGSMAX: u64 = 13015;

/// MSCONS "Redispatch 2.0 Ausfallarbeits-summenzeitreihe", AHB 3.2.
///
/// Same segment shape as the MaBiS Summenzeitreihe — a summed series over
/// settlement slots for one Zählpunkt — so it renders through the same path.
pub(super) const MSCONS_PID_AUSFALLARBEIT_SZR: u64 = 13023;

/// MSCONS "Werte nach Typ 2" (MSB → ESA), UC 4.2 / §60 Abs. 1 MsbG.
///
/// A MaLo + OBIS interval delivery addressed to the ESA (NAD+MR). Renders
/// through [`render_mscons_typ2`].
pub(super) const MSCONS_PID_WERTE_TYP2: u64 = 13027;

/// Render a summed MSCONS time series (Prüfidentifikator 13003 or 13023).
///
/// The payload carries the identifying 3-tuple — MaBiS-Zählpunkt,
/// Bilanzierungsmonat, Version (BK6-24-174 Anlage 3 §3.8.2) — and one entry per
/// settlement slot. MaBiS settles per quarter-hour, so the slots are the
/// message: a period total would carry the right sum and the wrong shape.
///
/// # Errors
///
/// [`RenderError::MissingField`] when the 3-tuple or the intervals are absent —
/// a Summenzeitreihe without them cannot be placed on the settlement grid.
pub(super) fn render_mscons(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "MSCONS";

    // MSCONS carries many Anwendungsfälle with materially different segment
    // shapes. Dispatching on the Prüfidentifikator keeps an unsupported one from
    // being rendered in the shape of a supported one, which would produce a
    // syntactically valid message stating something the sender did not mean.
    let pid = p
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    match pid {
        // Summenzeitreihe (MaBiS) and Redispatch 2.0
        // Ausfallarbeits-summenzeitreihe share the same shape: a summed series
        // over settlement slots for one Zählpunkt.
        MSCONS_PID_SUMMENZEITREIHE | MSCONS_PID_AUSFALLARBEIT_SZR => {}
        MSCONS_PID_ARBEIT_LEISTUNGSMAX
        | MSCONS_PID_ENERGIEMENGE
        | MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX => {
            return render_mscons_arbeit_leistungsmax(p, msg, registry);
        }
        MSCONS_PID_WERTE_TYP2 => {
            return render_mscons_typ2(p, msg, registry);
        }
        other => {
            return Err(RenderError::InsufficientPayload {
                message_type: mt.into(),
                detail: format!(
                    "MSCONS Prüfidentifikator {other} has no renderer. Supported: \
                     {MSCONS_PID_SUMMENZEITREIHE} (Summenzeitreihe), \
                     {MSCONS_PID_AUSFALLARBEIT_SZR} (Redispatch Ausfallarbeits-SZR), \
                     {MSCONS_PID_ARBEIT_LEISTUNGSMAX} (Arbeit/Leistungsmaximum), \
                     {MSCONS_PID_ENERGIEMENGE} (Energiemenge), \
                     {MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX} (Energiemenge + Leistungsmaximum), \
                     {MSCONS_PID_WERTE_TYP2} (Werte nach Typ 2, MSB→ESA)."
                )
                .into(),
            });
        }
    }

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    // SG6 carries three *different* LOC qualifiers (MSCONS AHB 3.2): `172` is
    // the Meldepunkt, `107` the Bilanzierungsgebiet, `237` the Bilanzkreis.
    //
    // What `172` holds depends on the use case: for the Summenzeitreihe family
    // it is the **MaBiS-Zählpunkt**, elsewhere the MaLo/MeLo. Both fields are
    // free text at the MIG level, so swapping them produces a message that
    // parses and validates while naming the wrong Meldepunkt.
    // Only the Summenzeitreihe PIDs reach here — the other MSCONS families
    // have their own renderers above and address a MaLo, not a MaBiS-ZP.
    let mabis_zp = p
        .get("mabis_zp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "mabis_zp_id (SG6 LOC+172 Meldepunkt — the MaBiS-Zählpunkt, not the Bilanzierungsgebiet)".into(),
        })?;
    let bilanzierungsgebiet = p
        .get("bilanzierungsgebiet_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "bilanzierungsgebiet_id (SG6 LOC+107)".into(),
        })?;
    // The two identify different things and can never share a value. Equality
    // means the Bilanzierungsgebiet was passed for both — the original defect,
    // refused here because it is invisible once on the wire.
    if bilanzierungsgebiet == mabis_zp {
        return Err(RenderError::BuilderError(format!(
            "MSCONS {pid}: mabis_zp_id and bilanzierungsgebiet_id are both \
             {bilanzierungsgebiet:?} — LOC+172 is the MaBiS-Zählpunkt and LOC+107 the \
             Bilanzierungsgebiet, so one value for both misidentifies the Meldepunkt"
        )));
    }
    let balancing_period = p
        .get("balancing_period")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "balancing_period (CCYYMM)".into(),
        })?;
    let version =
        p.get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: "version (CCYYMMDDHHMMSSZZZ)".into(),
            })?;

    let intervals = p
        .get("intervals")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "intervals".into(),
        })?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let mut mp = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(u32::try_from(pid).unwrap_or_default()).map_err(
                |e| RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}")),
            )?,
        )
        .metering_point(mabis_zp)
        .bilanzierungsgebiet(bilanzierungsgebiet)
        .balancing_period(balancing_period)
        .version(version);

    for iv in intervals {
        let (Some(from), Some(to), Some(qty)) = (
            iv.get("from").and_then(|v| v.as_str()),
            iv.get("to").and_then(|v| v.as_str()),
            iv.get("quantity_kwh").and_then(|v| v.as_str()),
        ) else {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "intervals[].{from,to,quantity_kwh}".into(),
            });
        };
        // DE 6063 `79` = "Energiemenge summiert (Summenwert, Bilanzsumme)";
        // DE 6411 `KWH` (MSCONS AHB 3.2, SG10 QTY). A consumption qualifier
        // would describe one metering point's draw, not the aggregate of a
        // Bilanzierungsgebiet.
        mp = mp.quantity_for_period(
            edi_energy::builders::QTY_ENERGIE_SUMMIERT,
            qty,
            "KWH",
            from,
            to,
        );
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render MSCONS "Arbeit / Leistungsmaximum im Kalenderjahr vor Lieferbeginn"
/// (Prüfidentifikator 13015).
///
/// Shape per AHB 3.2: SG9 repeats two to three times for one `NAD+DP` — once
/// for the energy from the start of the calendar year to Lieferbeginn, then
/// once or twice for the highest and second-highest monthly power maxima
/// (needed for the KAV concession-levy band).
///
/// Each maximum carries the period it fell in as `DTM+306`: format `610`
/// (`CCYYMM`) under a monthly or yearly Leistungspreissystem, `102`
/// (`CCYYMMDD`) under a daily one. A magnitude without that period cannot be
/// attributed to a month, which is what the KAV band depends on.
///
/// # Errors
///
/// [`RenderError::MissingField`] when the MaLo, the work entry or its period is
/// absent.
pub(super) fn render_mscons_arbeit_leistungsmax(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let pid = p
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    use edi_energy::builders::{
        MSCONS_UNITS, QTY_ERSATZWERT, QTY_WAHRER_WERT, is_valid_mscons_unit,
    };

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };
    // The AHB's per-Anwendungsfall table has no DE 6411 row for 13015, so the
    // unit follows the MIG's closed code list rather than a value fixed here:
    // the work entry is energy (`KWH`), a maximum is power (`KWT`).
    let checked_unit = |unit: &str| -> Result<(), RenderError> {
        if is_valid_mscons_unit(unit) {
            Ok(())
        } else {
            Err(RenderError::InsufficientPayload {
                message_type: mt.into(),
                detail: format!(
                    "unit {unit:?} is not a MSCONS DE 6411 code; expected one of {MSCONS_UNITS:?}"
                )
                .into(),
            })
        }
    };

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let malo_id = p
        .get("malo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| missing("malo_id"))?;

    let arbeit = p.get("arbeit").ok_or_else(|| missing("arbeit"))?;
    let (Some(arbeit_kwh), Some(from), Some(to)) = (
        arbeit.get("quantity").and_then(|v| v.as_str()),
        arbeit.get("from").and_then(|v| v.as_str()),
        arbeit.get("to").and_then(|v| v.as_str()),
    ) else {
        return Err(missing("arbeit.{quantity,from,to}"));
    };

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    // DE 6063 distinguishes a measured value from a substitute one. Reporting a
    // substitute as measured would assert a reading that was never taken.
    let qualifier = |v: &serde_json::Value| {
        if v.get("ersatzwert").and_then(serde_json::Value::as_bool) == Some(true) {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    let arbeit_unit = arbeit.get("unit").and_then(|v| v.as_str()).unwrap_or("KWH");
    checked_unit(arbeit_unit)?;

    let mut mp = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(u32::try_from(pid).unwrap_or_default()).map_err(
                |e| RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}")),
            )?,
        )
        .metering_point(malo_id)
        .quantity_for_period(qualifier(arbeit), arbeit_kwh, arbeit_unit, from, to);

    let maxima = p
        .get("leistungsmaxima")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();

    // 13019 is energy alone — the AHB marks no Leistungsperiode row for it, so
    // a maximum sent under it would have no period to be attributed to.
    if pid == MSCONS_PID_ENERGIEMENGE && !maxima.is_empty() {
        return Err(RenderError::InsufficientPayload {
            message_type: mt.into(),
            detail: format!(
                "Prüfidentifikator {MSCONS_PID_ENERGIEMENGE} carries Energiemenge only; \
                 send {MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX} to report a Leistungsmaximum"
            )
            .into(),
        });
    }

    // Up to two maxima. The AHB permits one or two; more would exceed the
    // segment-group repeat the message allows.
    if maxima.len() > 2 {
        return Err(RenderError::InsufficientPayload {
            message_type: mt.into(),
            detail: format!(
                "at most two Monatsleistungsmaxima may be sent, got {}",
                maxima.len()
            )
            .into(),
        });
    }

    for m in maxima {
        let (Some(value), Some(period)) = (
            m.get("quantity").and_then(|v| v.as_str()),
            m.get("period").and_then(|v| v.as_str()),
        ) else {
            return Err(missing("leistungsmaxima[].{quantity,period}"));
        };
        // `610` for a `CCYYMM` period, `102` for `CCYYMMDD` — the caller knows
        // which Leistungspreissystem applies.
        let period_format = m
            .get("period_format")
            .and_then(|v| v.as_str())
            .unwrap_or("610");
        // Power, not energy.
        let unit = m.get("unit").and_then(|v| v.as_str()).unwrap_or("KWT");
        checked_unit(unit)?;

        mp = mp
            .next_line_item()
            .quantity(qualifier(m), value, unit)
            .leistungsperiode(period, period_format);
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render an outbound MSCONS 13027 "Werte nach Typ 2" (MSB → ESA), UC 4.2.
///
/// A MaLo + OBIS interval delivery **addressed to the ESA** (NAD+MR = the ESA's
/// MP-ID from `receiver_mp_id`). Intervals are grouped by OBIS into line items;
/// each carries its quarter-hour quantities as Wirkarbeit (KWH). This is the
/// §60 Abs. 1 MsbG delivery duty on the wire — the values are non-authoritative
/// and land in the ESA's separate Typ-2 store.
pub(super) fn render_mscons_typ2(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::builders::{QTY_ERSATZWERT, QTY_WAHRER_WERT};

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };

    let sender = p
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| registry.primary_mp_id());
    // The recipient is the ESA — this is the whole point of the addressing gap:
    // the MSB emits 13027 to a party that is neither NB nor LF.
    let receiver = p
        .get("receiver_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let malo_id = p
        .get("malo_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| missing("malo_id"))?;

    let reads = p
        .get("reads")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| missing("reads"))?;

    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = active_release(MessageType::Mscons, ReleaseTrack::Short).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;

    let qualifier = |r: &serde_json::Value| {
        if r.get("ersatzwert").and_then(serde_json::Value::as_bool) == Some(true) {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    let mut builder = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(MSCONS_PID_WERTE_TYP2))
        // MSCONS AHB 3.2 §11.2: DE 2379 = `303` (`CCYYMMDDHHMMZZZ`), not the
        // date-only `102` the older use cases carry.
        .document_date_303()
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(
                u32::try_from(MSCONS_PID_WERTE_TYP2).unwrap_or_default(),
            )
            .map_err(|e| RenderError::BuilderError(format!("invalid Prüfidentifikator: {e}")))?,
        );

    // `SG1 RFF+AGI` — Muss on 13027, hint `[574]`: „Wert aus BGM DE1004 der
    // ORDERS mit der die Bestellung der Werte nach Typ 2 erfolgt ist". It ties
    // a delivery to the subscription that authorised it; the MSB-side process
    // carries the inbound Bestellung's Belegnummer.
    if let Some(bestellung) = p
        .get("korrelation_ref")
        .or_else(|| p.get("order_reference"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        builder = builder.header_reference("AGI", bestellung);
    }
    let mut mp = builder.metering_point(malo_id);

    // Group by OBIS into line items so one register's quarter-hour series lands
    // under one line item; a delivery without OBIS falls back to a bare item.
    let mut current_obis: Option<String> = None;
    let mut first_item = true;
    for r in reads {
        let (Some(quantity), Some(from), Some(to)) = (
            r.get("quantity_kwh").and_then(|v| v.as_str()),
            r.get("dtm_from").and_then(|v| v.as_str()),
            r.get("dtm_to").and_then(|v| v.as_str()),
        ) else {
            return Err(missing("reads[].{quantity_kwh,dtm_from,dtm_to}"));
        };
        let obis = r.get("obis_code").and_then(|v| v.as_str());
        if obis.map(str::to_owned) != current_obis {
            // Start a new line item for a new OBIS register.
            if !first_item {
                mp = mp.next_line_item();
            }
            if let Some(code) = obis {
                let parsed = rubo4e::identifiers::ObisCode::new(code).map_err(|e| {
                    RenderError::BuilderError(format!("invalid OBIS code {code:?}: {e}"))
                })?;
                mp = mp.line_item(parsed);
            }
            current_obis = obis.map(str::to_owned);
            first_item = false;
        }
        mp = mp.quantity_for_period(qualifier(r), quantity, "KWH", from, to);
    }

    super::finish_interchange_with_app_ref(
        mp.done().serialize(),
        sender,
        receiver,
        msg,
        // UNB DE 0026 = `TL` „Lastgang, beliebiger Zeitraum" — Muss on the
        // Werte-nach-Typ-2 interchange (MSCONS AHB 3.2 §11.2).
        Some("TL"),
    )
}
