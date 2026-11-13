//! MSCONS renderers (Summenzeitreihe, Energiemenge, Typ-2 Werte).
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;

/// MSCONS "Übertragung Summenzeitreihe" (MaBiS), AHB 3.2 Kap. 8.3.1.
pub(super) const MSCONS_PID_SUMMENZEITREIHE: u32 = 13003;

/// BGM DE 1001 document-name code for an MSCONS Anwendungsfall.
///
/// The code is not constant across MSCONS: it names what kind of document the
/// message is, and the AHB fixes a different one per use case. Sending the
/// wrong code labels a Summenzeitreihe as a Prozessdatenbericht, which the
/// receiver routes by.
pub(super) const fn mscons_document_code(pid: u32) -> &'static str {
    match pid {
        // "Zeitreihen im Rahmen der Bilanzkreisabrechnung"
        MSCONS_PID_SUMMENZEITREIHE => "BK",
        // "Redispatch"
        MSCONS_PID_AUSFALLARBEIT_SZR => "Z46",
        // "Bewegungsdaten im Kalenderjahr vor Lieferbeginn"
        MSCONS_PID_ARBEIT_LEISTUNGSMAX => "Z27",
        // "Energiemenge und Leistungsmaximum"
        MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX => "Z28",
        // "Werte nach Typ 2" (MSCONS AHB 3.2 Kap. 11.2). A `7`
        // („Prozessdatenbericht") here is refused by the generated rule
        // `AHB-13027-BGM-1001-Q`, which admits only `Z83`.
        MSCONS_PID_WERTE_TYP2 => "Z83",
        // "Lastgang Marktlokation, Tranche" (MSCONS AHB 3.2 Kap. 7.2.2, S. 69)
        MSCONS_PID_LASTGANG_MALO => "Z48",
        // "Prozessdatenbericht"
        _ => "7",
    }
}

/// MSCONS "Energiemenge (Strom)", AHB 3.2 — energy for a billing period, with
/// no power maximum.
pub(super) const MSCONS_PID_ENERGIEMENGE: u32 = 13019;

/// MSCONS "Energiemenge und Leistungsmaximum", AHB 3.2.
pub(super) const MSCONS_PID_ENERGIEMENGE_LEISTUNGSMAX: u32 = 13016;

/// MSCONS "Arbeit / Leistungsmaximum im Kalenderjahr vor Lieferbeginn",
/// AHB 3.2 — the movement data a Netznutzungsvertrag requires when an RLM
/// Marktlokation changes supplier mid-year (GPKE Kap. 6.1).
pub(super) const MSCONS_PID_ARBEIT_LEISTUNGSMAX: u32 = 13015;

/// MSCONS "Redispatch 2.0 Ausfallarbeits-summenzeitreihe", AHB 3.2.
///
/// Same segment shape as the MaBiS Summenzeitreihe — a summed series over
/// settlement slots for one Zählpunkt — so it renders through the same path.
pub(super) const MSCONS_PID_AUSFALLARBEIT_SZR: u32 = 13023;

/// MSCONS "Werte nach Typ 2" (MSB → ESA), UC 4.2 / §60 Abs. 1 MsbG.
///
/// A MaLo + OBIS interval delivery addressed to the ESA (NAD+MR). Renders
/// through [`render_mscons_werte`].
pub(super) const MSCONS_PID_WERTE_TYP2: u32 = 13027;

/// MSCONS "Lastgang Messlokation, Netzkoppelpunkt, Netzlokation" (MSB → LF on
/// an Anfrage von Werten, GPKE Teil 4), MSCONS AHB 3.2 Kap. 7.2.2.
pub(super) const MSCONS_PID_LASTGANG_MELO: u32 = 13018;

/// MSCONS "Lastgang Marktlokation, Tranche", MSCONS AHB 3.2 Kap. 7.2.2 — the
/// same column as 13018.
pub(super) const MSCONS_PID_LASTGANG_MALO: u32 = 13025;

/// MSCONS "Zählerstand (Strom)" — the MSB's Zählerstände (MSCONS AHB 3.2 Kap.
/// 6.3.1, S. 23–29). Renders through [`render_mscons_zaehlerstand`].
pub(super) const MSCONS_PID_ZAEHLERSTAND: u32 = 13017;

/// Render a summed MSCONS time series (Prüfidentifikator 13003 or 13023).
///
/// The payload carries the identifying 3-tuple — MaBiS-Zählpunkt,
/// Bilanzierungsmonat, Version (BK6-24-174 Anlage 3 Kap. 3.8.2) — and one entry per
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
    let typed: mako_engine::payload::MsconsPayload = super::typed_payload(p)?;
    let p = &typed;

    // MSCONS carries many Anwendungsfälle with materially different segment
    // shapes. Dispatching on the Prüfidentifikator keeps an unsupported one from
    // being rendered in the shape of a supported one, which would produce a
    // syntactically valid message stating something the sender did not mean.
    let pid = super::checked_pid(p.pid, "MSCONS")?.unwrap_or(0);
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
        MSCONS_PID_WERTE_TYP2 | MSCONS_PID_LASTGANG_MELO | MSCONS_PID_LASTGANG_MALO => {
            return render_mscons_werte(pid, p, msg, registry);
        }
        MSCONS_PID_ZAEHLERSTAND => {
            return render_mscons_zaehlerstand(p, msg, registry);
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
                     {MSCONS_PID_WERTE_TYP2} (Werte nach Typ 2, MSB→ESA), \
                     {MSCONS_PID_LASTGANG_MELO} / {MSCONS_PID_LASTGANG_MALO} (Lastgang), \
                     {MSCONS_PID_ZAEHLERSTAND} (Zählerstand)."
                )
                .into(),
            });
        }
    }

    let sender = text(&p.sender_mp_id).unwrap_or_else(|| registry.primary_mp_id());
    let receiver = text(&p.receiver_mp_id).unwrap_or(msg.recipient.as_ref());

    // SG6 carries three *different* LOC qualifiers (MSCONS AHB 3.2): `172` is
    // the Meldepunkt, `107` the Bilanzierungsgebiet, `237` the Bilanzkreis.
    //
    // What `172` holds depends on the use case: for the Summenzeitreihe family
    // it is the **MaBiS-Zählpunkt**, elsewhere the MaLo/MeLo. Both fields are
    // free text at the MIG level, so swapping them produces a message that
    // parses and validates while naming the wrong Meldepunkt.
    // Only the Summenzeitreihe PIDs reach here — the other MSCONS families
    // have their own renderers above and address a MaLo, not a MaBiS-ZP.
    let mabis_zp = text(&p.mabis_zp_id)
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "mabis_zp_id (SG6 LOC+172 Meldepunkt — the MaBiS-Zählpunkt, not the Bilanzierungsgebiet)".into(),
        })?;
    let balancing_period = text(&p.balancing_period).ok_or_else(|| RenderError::MissingField {
        message_type: mt.into(),
        field: "balancing_period (CCYYMM)".into(),
    })?;
    let version = text(&p.version).ok_or_else(|| RenderError::MissingField {
        message_type: mt.into(),
        field: "version (CCYYMMDDHHMMSSZZZ)".into(),
    })?;

    // `SG9 PIA+5+<OBIS>:SRW` is Muss on every MSCONS position; the
    // Summenzeitreihe's OBIS-Kennzahl names the series.
    let obis = text(&p.obis_code)
        .ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "obis_code (SG9 PIA+5+<OBIS>:SRW)".into(),
        })
        .and_then(|code| {
            rubo4e::identifiers::ObisCode::new(code)
                .map_err(|e| RenderError::BuilderError(format!("invalid OBIS code {code:?}: {e}")))
        })?;
    if p.intervals.is_empty() {
        return Err(RenderError::MissingField {
            message_type: mt.into(),
            field: "intervals".into(),
        });
    }

    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Mscons, ReleaseTrack::Short)?;

    // `QTY` DE 6411 goes out where the column lists it (13023 admits `KWH`,
    // 13003 lists none — the unit is the OBIS-Kennzahl's).
    let unit = if super::column_lists(MessageType::Mscons, &release, pid, "QTY", "6411") {
        "KWH"
    } else {
        ""
    };
    let mut mp = builders::MsconsBuilder::new(release.clone())
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(edi_energy::Pruefidentifikator::new(pid).map_err(|e| {
            RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}"))
        })?)
        .metering_point(mabis_zp)
        .balancing_period(balancing_period)
        .version(version)
        .line_item(obis);
    if pid == MSCONS_PID_AUSFALLARBEIT_SZR {
        // The Redispatch Ausfallarbeits-SZR names its series as a Medium
        // (`PIA+5+…:Z08`), not as an OBIS-Kennzahl.
        mp = mp.pia_type("Z08");
    }

    for iv in &p.intervals {
        let (from, to, qty) = (iv.from.as_str(), iv.to.as_str(), iv.quantity_kwh.as_str());
        // DE 6063 `79` = "Energiemenge summiert (Summenwert, Bilanzsumme)";
        // DE 6411 `KWH` (MSCONS AHB 3.2, SG10 QTY). A consumption qualifier
        // would describe one metering point's draw, not the aggregate of a
        // Bilanzierungsgebiet.
        mp = mp.quantity_for_period(
            edi_energy::builders::QTY_ENERGIE_SUMMIERT,
            qty,
            unit,
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
    p: &mako_engine::payload::MsconsPayload,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let pid = super::checked_pid(p.pid, "MSCONS")?.unwrap_or(0);
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

    let sender = text(&p.sender_mp_id).unwrap_or_else(|| registry.primary_mp_id());
    let receiver = text(&p.receiver_mp_id).unwrap_or(msg.recipient.as_ref());
    let malo_id = text(&p.malo_id).ok_or_else(|| missing("malo_id"))?;

    let arbeit = p.arbeit.as_ref().ok_or_else(|| missing("arbeit"))?;
    let (arbeit_kwh, Some(from), Some(to)) = (
        arbeit.quantity.as_str(),
        text(&arbeit.from),
        text(&arbeit.to),
    ) else {
        return Err(missing("arbeit.{quantity,from,to}"));
    };

    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Mscons, ReleaseTrack::Short)?;

    // DE 6063 distinguishes a measured value from a substitute one. Reporting a
    // substitute as measured would assert a reading that was never taken.
    let qualifier = |v: &mako_engine::payload::MsconsWert| {
        if v.ersatzwert {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    // No column lists DE 6411 on `QTY`; a unit goes out only when stated.
    let arbeit_unit = arbeit.unit.as_deref().unwrap_or("");
    if !arbeit_unit.is_empty() {
        checked_unit(arbeit_unit)?;
    }
    let obis_of = |v: &mako_engine::payload::MsconsWert,
                   default: &str|
     -> Result<rubo4e::identifiers::ObisCode, RenderError> {
        let code = v.obis.as_deref().unwrap_or(default);
        rubo4e::identifiers::ObisCode::new(code)
            .map_err(|e| RenderError::BuilderError(format!("invalid OBIS code {code:?}: {e}")))
    };
    // `SG1 RFF+AGI` — the Bestellung/Anfrage this answers, Muss on 13015,
    // 13016 and 13019.
    let order_reference =
        text(&p.korrelation_ref).ok_or_else(|| missing("korrelation_ref (SG1 RFF+AGI)"))?;
    // `STS+Z40` Grund der Ersatzwertbildung is a place 13016/13019 list and
    // 13015 does not.
    let lists_grund = super::column_lists_place(
        MessageType::Mscons,
        &release,
        pid,
        "STS",
        Some("Z40"),
        "9013",
    );
    let ersatzwert_of =
        |v: &mako_engine::payload::MsconsWert| -> Result<Option<(String, String)>, RenderError> {
            if !v.ersatzwert {
                return Ok(None);
            }
            let verfahren = text(&v.ersatzwert_verfahren)
                .ok_or_else(|| missing("ersatzwert_verfahren (STS+Z32: Z88/Z89/Z92/ZJ2)"))?;
            let grund = if lists_grund {
                text(&v.ersatzwert_grund)
                    .ok_or_else(|| missing("ersatzwert_grund (STS+Z40: Z74/Z75/Z76/Z77)"))?
            } else {
                ""
            };
            Ok(Some((verfahren.to_owned(), grund.to_owned())))
        };

    let mut mp = builders::MsconsBuilder::new(release.clone())
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(pid))
        .pruefidentifikator(edi_energy::Pruefidentifikator::new(pid).map_err(|e| {
            RenderError::BuilderError(format!("invalid Prüfidentifikator {pid}: {e}"))
        })?)
        .header_reference("AGI", order_reference)
        .metering_point(malo_id)
        // The Energiemenge is OBIS `1-1:1.9.0` (Wirkarbeit); Bedingungen
        // [27]/[73] tie the Messperiode `DTM+163`/`164` to it.
        .line_item(obis_of(arbeit, "1-1:1.9.0")?)
        .quantity_for_period(qualifier(arbeit), arbeit_kwh, arbeit_unit, from, to);
    if let Some((verfahren, grund)) = ersatzwert_of(arbeit)? {
        mp = mp.ersatzwert(verfahren, grund);
    }
    if pid == MSCONS_PID_ENERGIEMENGE {
        // `SG6 RFF+AGK` Konfigurations-ID and `SG10 STS+10` Messklassifizierung
        // — Muss on the Energiemenge (13019).
        if let Some(k) = text(&p.messklassifizierung) {
            mp = mp.messklassifizierung(k);
        }
    }

    let maxima = p.leistungsmaxima.as_slice();

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
        let (value, Some(period)) = (m.quantity.as_str(), text(&m.period)) else {
            return Err(missing("leistungsmaxima[].{quantity,period}"));
        };
        // `610` for a `CCYYMM` period, `102` for `CCYYMMDD` — the caller knows
        // which Leistungspreissystem applies.
        let period_format = m.period_format.as_deref().unwrap_or("610");
        // Power, not energy.
        let unit = m.unit.as_deref().unwrap_or("");
        if !unit.is_empty() {
            checked_unit(unit)?;
        }
        // A Leistungsmaximum is OBIS `1-1:1.6.0`; Bedingung [28]/[72] tie
        // the Leistungsperiode `DTM+306` to it.
        mp = mp
            .next_line_item()
            .line_item(obis_of(m, "1-1:1.6.0")?)
            .quantity(qualifier(m), value, unit)
            .leistungsperiode(period, period_format);
        if let Some((verfahren, grund)) = ersatzwert_of(m)? {
            mp = mp.ersatzwert(verfahren, grund);
        }
    }

    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render an MSCONS 13017 „Zählerstand (Strom)" (MSCONS AHB 3.2 Kap. 6.3.1,
/// S. 23–29): `BGM+7`, `SG1 RFF+AGI` the Anfrage, `SG6 LOC+172` the
/// Messlokation with `SG7 RFF+MG` its Zähler — or for an iMS no Lokation and
/// `SG7 RFF+AGK` — and per Stand one `SG9 LIN`/`PIA`, `SG10 QTY`, `DTM+9`
/// Ablesedatum where stated, `DTM+7` Gültigkeitsdatum and on an Ersatzwert
/// `STS+Z32`/`Z40`.
pub(super) fn render_mscons_zaehlerstand(
    p: &mako_engine::payload::MsconsPayload,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::builders::{QTY_ERSATZWERT, QTY_WAHRER_WERT};

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };
    let sender = text(&p.sender_mp_id).unwrap_or_else(|| registry.primary_mp_id());
    let receiver = text(&p.receiver_mp_id).unwrap_or(msg.recipient.as_ref());
    // `SG6 LOC+172` names the Messlokation with its Zähler in `SG7 RFF+MG`,
    // or — for values from an iMS — nothing, the Konfigurations-ID riding
    // `SG7 RFF+AGK` (`[131]`, `[132]`). Exactly one of the two.
    let (lokation, geraet, konfiguration) =
        match (text(&p.geraetenummer), text(&p.konfigurations_id)) {
            (Some(g), None) => (
                text(&p.malo_id)
                    .ok_or_else(|| missing("malo_id (SG6 LOC+172 — the Messlokation)"))?,
                Some(g),
                None,
            ),
            (None, Some(k)) => ("", None, Some(k)),
            _ => {
                return Err(missing(
                    "geraetenummer (SG7 RFF+MG) xor konfigurations_id (SG7 RFF+AGK)",
                ));
            }
        };
    if p.zaehlerstaende.is_empty() {
        return Err(missing("zaehlerstaende"));
    }
    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));
    let release = release_for(msg, MessageType::Mscons, ReleaseTrack::Short)?;

    let mut builder = builders::MsconsBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref)
        .document_code(mscons_document_code(MSCONS_PID_ZAEHLERSTAND))
        .pruefidentifikator(
            edi_energy::Pruefidentifikator::new(MSCONS_PID_ZAEHLERSTAND).map_err(|e| {
                RenderError::BuilderError(format!("invalid Prüfidentifikator: {e}"))
            })?,
        );
    // `SG1 RFF+AGI` — the ORDERS that asked (Soll `[1]`, `[529]`).
    if let Some(anfrage) = text(&p.korrelation_ref) {
        builder = builder.header_reference("AGI", anfrage);
    }
    let mut mp = builder.metering_point(lokation).location_id(lokation);
    if let Some(g) = geraet {
        mp = mp.geraetenummer(g);
    }
    if let Some(k) = konfiguration {
        mp = mp.konfigurations_id(k);
    }
    for (i, st) in p.zaehlerstaende.iter().enumerate() {
        let obis = rubo4e::identifiers::ObisCode::new(&st.obis_code).map_err(|e| {
            RenderError::BuilderError(format!("invalid OBIS code {:?}: {e}", st.obis_code))
        })?;
        if i > 0 {
            mp = mp.next_line_item();
        }
        // `SG10 DTM+7` Gültigkeitsdatum — `[UB1]`: the instant the German day
        // begins; `DTM+9` Ablesedatum is the day itself, `102`.
        let gueltig = prozessdatum_303(mt, "gueltig", &st.gueltig, mako_fristen::Sparte::Strom)?;
        let ablesedatum = st.ablesedatum.as_deref().map(normalise_date);
        mp = mp.line_item(obis).zaehlerstand(
            if st.ersatzwert {
                QTY_ERSATZWERT
            } else {
                QTY_WAHRER_WERT
            },
            &st.quantity,
            gueltig,
            ablesedatum,
        );
        // `STS+Z32` / `STS+Z40` — Muss beside a `QTY+67` (`[92]`).
        if st.ersatzwert {
            mp = mp.ersatzwert(
                text(&st.ersatzwert_verfahren)
                    .ok_or_else(|| missing("zaehlerstaende[].ersatzwert_verfahren (STS+Z32)"))?,
                text(&st.ersatzwert_grund)
                    .ok_or_else(|| missing("zaehlerstaende[].ersatzwert_grund (STS+Z40)"))?,
            );
        }
    }
    finish_interchange(mp.done().serialize(), sender, receiver, msg)
}

/// Render an outbound series of interval values: MSCONS 13027 "Werte nach
/// Typ 2" (MSB → ESA, UC 4.2) or a Lastgang 13018 / 13025 (MSCONS AHB 3.2 Kap.
/// 7.2.2, S. 69–75).
///
/// 13027 is a MaLo + OBIS interval delivery **addressed to the ESA** (NAD+MR =
/// the ESA's MP-ID from `receiver_mp_id`), the §60 Abs. 1 MsbG delivery duty
/// on the wire. Intervals are grouped by OBIS into line items; each carries its
/// quantities. A Lastgang additionally states its Messperiode in `SG6
/// DTM+163`/`164` (Muss), the first value's start to the last one's end.
pub(super) fn render_mscons_werte(
    pid: u32,
    p: &mako_engine::payload::MsconsPayload,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::builders::{QTY_ERSATZWERT, QTY_WAHRER_WERT};

    let mt = "MSCONS";
    let missing = |field: &str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };

    let sender = text(&p.sender_mp_id).unwrap_or_else(|| registry.primary_mp_id());
    // The recipient is the ESA — this is the whole point of the addressing gap:
    // the MSB emits 13027 to a party that is neither NB nor LF.
    let receiver = text(&p.receiver_mp_id).unwrap_or(msg.recipient.as_ref());
    let malo_id = text(&p.malo_id).ok_or_else(|| missing("malo_id"))?;

    if p.reads.is_empty() {
        return Err(missing("reads"));
    }

    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Mscons, ReleaseTrack::Short)?;

    let qualifier = |r: &mako_engine::payload::MsconsRead| {
        if r.ersatzwert {
            QTY_ERSATZWERT
        } else {
            QTY_WAHRER_WERT
        }
    };

    // `QTY` DE 6411 goes out where the column lists it.
    let unit = if super::column_lists(MessageType::Mscons, &release, pid, "QTY", "6411") {
        "KWH"
    } else {
        ""
    };
    let mut builder =
        builders::MsconsBuilder::new(release)
            .sender(sender)
            .receiver(receiver)
            .message_ref(message_ref)
            .document_code(mscons_document_code(pid))
            .pruefidentifikator(edi_energy::Pruefidentifikator::new(pid).map_err(|e| {
                RenderError::BuilderError(format!("invalid Prüfidentifikator: {e}"))
            })?);

    // `SG1 RFF+AGI` — Muss on 13027, hint `[574]`: „Wert aus BGM DE1004 der
    // ORDERS mit der die Bestellung der Werte nach Typ 2 erfolgt ist". It ties
    // a delivery to the subscription that authorised it; the MSB-side process
    // carries the inbound Bestellung's Belegnummer.
    if let Some(bestellung) = text(&p.korrelation_ref) {
        builder = builder.header_reference("AGI", bestellung);
    }
    let mut mp = builder.metering_point(malo_id);
    if pid != MSCONS_PID_WERTE_TYP2 {
        let beginn = p.reads.iter().map(|r| r.dtm_from.as_str()).min();
        let ende = p.reads.iter().map(|r| r.dtm_to.as_str()).max();
        if let (Some(beginn), Some(ende)) = (beginn, ende) {
            mp = mp.messperiode(beginn, ende);
        }
    }

    // Group by OBIS into line items so one register's quarter-hour series lands
    // under one line item; a delivery without OBIS falls back to a bare item.
    let mut current_obis: Option<String> = None;
    let mut first_item = true;
    for r in &p.reads {
        let (quantity, from, to) = (
            r.quantity_kwh.as_str(),
            r.dtm_from.as_str(),
            r.dtm_to.as_str(),
        );
        let obis = text(&r.obis_code);
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
        mp = mp.quantity_for_period(qualifier(r), quantity, unit, from, to);
    }

    super::finish_interchange_with_app_ref(
        mp.done().serialize(),
        sender,
        receiver,
        msg,
        // UNB DE 0026 = `TL` „Lastgang, beliebiger Zeitraum" — Muss on the
        // Werte-nach-Typ-2 interchange (MSCONS AHB 3.2 Kap. 11.2) and on the
        // Lastgang (Kap. 7.2.2, S. 69).
        Some("TL"),
    )
}
