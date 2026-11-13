//! ORDERS / REQOTE / ORDCHG / ORDRSP / QUOTES renderers.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;

// ── ESA Wertebestellung helpers (WiM Strom Teil 2 Kap. 4) ────────────────────

/// Emit the `RFF` that correlates an ESA-Wertebestellung message, under the
/// qualifier the BDEW *Anwendungsübersicht der Prüfidentifikatoren* 4.0
/// publishes for that PID.
///
/// The qualifier is **not** interchangeable: `RFF+ON` on an ORDRSP 19011 and
/// `RFF+ACW` on an ORDRSP 19013 point at different messages (the ORDERS and
/// the ORDCHG respectively), and a receiver keying on the wrong one finds no
/// process. [`mako_wim::esa::korrelation`] is that table; reading the
/// qualifier from it is what keeps the renderer and the ingest dispatcher from
/// drifting apart.
fn esa_korrelation_qualifier(pid: Option<u32>) -> Option<&'static str> {
    pid.and_then(mako_wim::esa::korrelation)
        .and_then(mako_wim::esa::Korrelation::rff_qualifier)
}

/// The `303` value of an ESA process date (REQOTE `DTM+76` Wunschtermin,
/// ORDERS `DTM+203` Ausführungsdatum, QUOTES `DTM+469` frühester Start).
///
/// The ESA is a Strom role (Rollenmodell V2.2), so the date goes out as the
/// UTC instant of 00:00 Europe/Berlin — see [`prozessdatum_303`].
fn esa_dtm303(message_type: &str, field: &str, date: &str) -> Result<String, RenderError> {
    prozessdatum_303(message_type, field, date, mako_fristen::Sparte::Strom)
}

// ── ORDERS ────────────────────────────────────────────────────────────────────

/// Render an ESA-originated REQOTE 35003 Werteanfrage (WiM Teil 2 UC 4.1 Nr. 1).
///
/// The PID travels in BGM DE 1004 (`document_id`) and the addressed location in
/// `LOC+172`, so the MSB can correlate the request to a Marktlokation.
pub(super) fn render_reqote(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "REQOTE";
    let p: mako_engine::payload::ReqotePayload = super::typed_payload(p)?;

    let pid = checked_pid(p.pid, mt)?;

    let sender = p
        .sender
        .as_deref()
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p.receiver.as_deref().unwrap_or(msg.recipient.as_ref());

    let explicit_ref = p.message_ref.as_deref().map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    let release = release_for(msg, MessageType::Reqote, ReleaseTrack::Short)?;

    let mut builder = builders::ReqoteBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    if let Some(pv) = pid {
        // `SG1 RFF+Z13`; BGM DE 1004 keeps the Belegnummer.
        builder = builder.pruefidentifikator(pv);
    }
    if let Some(loc) = text(&p.location) {
        builder = builder.location(loc);
    }

    // ── ESA Werteanfrage (PID 35003) full-conformance content ────────────────
    //
    // REQOTE AHB 1.2 Kap. 4.3. Muss beyond the shared skeleton: `BGM+Z57`,
    // `DTM+76` (der Wunschtermin — WiM Teil 2 UC 4.1.2 Nr. 1: „Der ESA gibt
    // u. a. seinen Wunschtermin für die erstmalige Übermittlung von Werten
    // mit"), `SG1 RFF+Z13`, `SG14 CTA/COM`, `SG11 NAD+DP` before the
    // `LOC+172` Meldepunkt, and one `SG27 LIN+Z67` with `PIA+5+<code>:Z11`
    // naming a Messprodukt from Codeliste der Konfigurationen Kapitel 4.6.1.
    //
    // `FTX+ACB` is a *Kann* segment and is emitted only when the caller
    // supplied a note — the builder drops an empty one rather than putting a
    // blank free text on the wire.
    if pid == Some(35003) {
        builder = builder.document_code("Z57").delivery_party();
        if let Some(termin) = text(&p.wunschtermin) {
            builder = builder.leistungsbeginn(esa_dtm303(mt, "wunschtermin", termin)?);
        }
        let contact = text(&p.contact).unwrap_or("ESA-Service");
        let comm = text(&p.contact_comm).unwrap_or_else(|| registry.primary_mp_id());
        builder = builder.contact(contact, comm);
        if let Some(note) = text(&p.bemerkung) {
            builder = builder.free_text("ACB", note);
        }
        // The Messprodukt is what makes this message a Werteanfrage, and DE
        // 7140 accepts only Kapitel-4.6 codes — so it is resolved against the
        // catalogue rather than defaulted.
        let Some(messprodukt) = text(&p.messprodukt) else {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "messprodukt".into(),
            });
        };
        let produkt = mako_wim::esa::messprodukt(messprodukt).ok_or(RenderError::MissingField {
            message_type: mt.into(),
            field: "messprodukt (nicht in Codeliste der Konfigurationen Kapitel 4.6)".into(),
        })?;
        builder = builder.product(produkt.weg.lin_code(), produkt.code);
        // Kapitel 4.6.2 products additionally carry the SM-PKI delivery target
        // (`FTX+Z17/Z24/Z23`) and, when Schwellwert-triggered, `SG28 CCI+Z60`.
        if let Some(ziel) = &p.smgw {
            builder = builder.smgw_delivery(
                &ziel.uri_ipv4,
                &ziel.uri_ipv6,
                &ziel.zertifikat_aussteller,
                &ziel.zertifikat_nutzer,
            );
            for sw in &ziel.schwellwerte {
                builder = builder.schwellwert(&sw.position_code, &sw.oberer, &sw.unterer);
            }
        }
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// Render an ORDERS (Beauftragung) message from domain-intent JSON.
///
/// **Sender resolution** (in priority order):
/// 1. `payload["sender"]` — set this in the workflow for deterministic
///    multi-MP-ID deployments.
/// 2. [`MpIdRegistry::sender_mp_id_for_orders_pid`], narrowed by the payload's
///    `sparte` where the PID is shared between Strom and Gas.
/// 3. [`MpIdRegistry::primary_mp_id`] — final fallback.
///
/// The receiver comes from `msg.recipient`.
///
/// Payload fields:
///
/// | Field        | Required | Description                                  |
/// |--------------|----------|----------------------------------------------|
/// | `sender`     | no       | Sender MP-ID (overrides registry lookup)       |
/// | `pid`        | no       | ORDERS Prüfidentifikator (e.g. 17134)        |
/// | `orders_ref` | no       | UUID reference → 14-char UNH message ref     |
/// | `document_code` | no    | BGM DE 1001 (`Z14` on 17132)                 |
/// | `document_id` | no      | BGM DE 1004 — the Anfrage-Nummer on 17132    |
/// | `location`   | no       | `SG2 LOC+172`, opened by `NAD+DP`            |
pub(super) fn render_orders(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDERS";
    let p: mako_engine::payload::OrdersPayload = super::typed_payload(p)?;

    let pid = checked_pid(p.pid, mt)?;

    // The Sparte a handful of shared PIDs need to pick between two Marktrollen.
    // The emitting workflow states it; a payload without one leaves the lookup
    // to weigh the two roles and warn.
    let sparte = p.sparte;

    // Sender: explicit in payload first, then registry lookup by PID, then primary.
    let sender = p.sender.as_deref().unwrap_or_else(|| {
        pid.map(|p| registry.sender_mp_id_for_orders_pid(p, sparte))
            .unwrap_or_else(|| registry.primary_mp_id())
    });

    // Prefer the caller's Belegnummer (`message_ref`) so the wire UNH reference
    // matches the key the ESA registered its process under — the MSB's ORDRSP
    // answer echoes this reference and it is how a LOC-less answer correlates.
    let explicit_ref = text(&p.message_ref).map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    let release = release_for(msg, MessageType::Orders, ReleaseTrack::Short)?;

    let mut builder = builders::OrdersBuilder::new(release)
        .sender(sender)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        // `SG1 RFF+Z13`; BGM DE 1004 keeps the Belegnummer.
        builder = builder.pruefidentifikator(pv);
    }
    // `BGM` DE 1001 and DE 1004 where the workflow states them — the
    // Anfrage Stammdaten 17132 is `BGM+Z14` with the Anfrage-Nummer the NB's
    // answer echoes (ORDERS AHB 1.1b Kap. 4.1.1.1).
    if let Some(code) = text(&p.document_code) {
        builder = builder.document_code(code);
    }
    if let Some(id) = text(&p.document_id) {
        builder = builder.document_id(id);
    }
    // A `LOC` is emitted only where the AHB has one. Kap. 4.15 of the ORDERS AHB
    // lists **no** Meldepunkt segment for 17007/17008, so the ESA order PIDs
    // never carry one — the workflow already leaves `location` null for them,
    // and this guard keeps a hand-written payload from re-introducing it.
    let ist_esa_order = pid == Some(17007) || pid == Some(17008);
    if let Some(loc) = text(&p.location)
        && !ist_esa_order
    {
        builder = builder.location(loc);
    }
    // ── ESA Bestellung/Abbestellung (17007/17008) full conformance ───────────
    //
    // ORDERS AHB 1.1b Kap. 4.15. Muss: `BGM+Z57` („Übermittlung von Werten an
    // ESA"), `DTM+203` Ausführungsdatum, `IMD++<7081>` (Z01 Start Abo / Z02
    // Ende Abo / Z03 ohne Abo), `SG1 RFF+Z13`, and the correlation reference —
    // `RFF+AAG` (the QUOTES Angebotsnummer) on 17007, `RFF+ACW` (the ORDERS
    // Bestellnummer) on 17008. Without the latter two the MSB has no way at all
    // to match the order: these messages carry no location.
    if ist_esa_order {
        builder = builder.document_code("Z57");
        if let (Some(qual), Some(reference)) =
            (esa_korrelation_qualifier(pid), text(&p.korrelation_ref))
        {
            builder = builder.reference(qual, reference);
        }
        if let Some(datum) = text(&p.ausfuehrungsdatum) {
            builder = builder.ausfuehrungsdatum(esa_dtm303(mt, "ausfuehrungsdatum", datum)?);
        }
        if let Some(abo) = text(&p.abonnement) {
            builder = builder.abonnement(abo);
        }
    }
    // ── Anfrage Brennwert und Zustandszahl (17103) ───────────────────────────
    //
    // ORDERS AHB 1.1b Kap. 4.1.3. Muss: `BGM+7` (Prozessdatenbericht),
    // `IMD++Z10` (Abrechnungsbrennwert und Zustandszahl), the Messlokation in
    // `LOC+172`, and one `SG29 LIN` carrying the Zeitspanne as `DTM+163` /
    // `DTM+164`. The Zeitspanne's end is what the GNB's answer window is
    // computed from (GeLi Gas 3.0 Kap. 4.2), so an ORDERS without it would
    // leave the GNB unable to register the window it owes — refused, never
    // rendered without. A Gas process date is the instant its Gastag begins
    // (Allgemeine Festlegungen V6.1d Kap. 3.1), and the Ende is the beginning
    // of the Gastag after the last one.
    if pid == Some(17103) {
        let tag = |field: &str| -> Result<time::Date, RenderError> {
            let missing = || RenderError::MissingField {
                message_type: mt.into(),
                field: field.into(),
            };
            let v = match field {
                "zeitspanne_von" => text(&p.zeitspanne_von),
                _ => text(&p.zeitspanne_bis),
            }
            .ok_or_else(missing)?;
            time::Date::parse(v, time::macros::format_description!("[year]-[month]-[day]"))
                .map_err(|_| missing())
        };
        if text(&p.location).is_none() {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "location (Messlokation)".into(),
            });
        }
        let von = tag("zeitspanne_von")?;
        let bis = tag("zeitspanne_bis")?;
        let ende = bis.next_day().ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "zeitspanne_bis".into(),
        })?;
        builder = builder
            .document_code("7")
            .leistung("Z10")
            .positionszeitraum(
                ccyymmddhhmm(mako_fristen::tagesbeginn_utc(
                    von,
                    mako_fristen::Sparte::Gas,
                )),
                ccyymmddhhmm(mako_fristen::tagesbeginn_utc(
                    ende,
                    mako_fristen::Sparte::Gas,
                )),
            );
    }

    builder = orders_spalte(builder, pid, &p, mt)?;

    finish_interchange(builder.serialize(), sender, msg.recipient.as_ref(), msg)
}

/// The Sparte a process date is a day boundary of.
fn payload_sparte(
    sparte: Option<mako_engine::types::Sparte>,
    mt: &'static str,
    detail: &'static str,
) -> Result<mako_fristen::Sparte, RenderError> {
    match sparte {
        Some(mako_engine::types::Sparte::Strom) => Ok(mako_fristen::Sparte::Strom),
        Some(mako_engine::types::Sparte::Gas) => Ok(mako_fristen::Sparte::Gas),
        None => Err(RenderError::MissingField {
            message_type: mt.into(),
            field: detail.into(),
        }),
    }
}

/// An Ansprechpartner's name and its `COM` channels as `(address, code)`.
type Kontakt = (String, Vec<(String, &'static str)>);

/// `CTA`/`COM` channels of an [`Ansprechpartner`].
///
/// [`Ansprechpartner`]: mako_engine::payload::Ansprechpartner
fn kommunikationswege(
    kontakt: &mako_engine::payload::Ansprechpartner,
    mt: &'static str,
) -> Result<Kontakt, RenderError> {
    let kanaele = kontakt
        .kommunikation
        .iter()
        .map(|k| {
            let code = match k.art.as_str() {
                "email" => "EM",
                "telefon" => "TE",
                "telefax" => "FX",
                "weiteres_telefon" => "AJ",
                "handy" => "AL",
                other => {
                    return Err(RenderError::InsufficientPayload {
                        message_type: mt.into(),
                        detail: format!("ansprechpartner: unknown COM channel {other:?}").into(),
                    });
                }
            };
            Ok((k.adresse.clone(), code))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((kontakt.name.clone(), kanaele))
}

/// The ORDERS columns whose body is more than the head:
///
/// | PID | Column (ORDERS AHB 1.1b) | Payload |
/// |---|---|---|
/// | 17001 | Kap. 4.9.2, S. 74–77: `BGM+Z10`, `DTM+203`, `SG1 RFF+AAG`, `SG5 CTA`/`COM`, one `SG29 LIN` with `SG34 RFF+Z03` per Angebotsposition | `angebotsnummer`, `ausfuehrungsdatum`, `sparte`, `ansprechpartner`, `positionen` |
/// | 17009 | Kap. 4.10, S. 78–80: `BGM+Z13`, `DTM+203`, `IMD++Z58`/`Z59`, `LOC+172`, one `SG29 LIN` with `SG30 CCI` and `SG34 RFF+Z09` per Gerät | `location`, `ausfuehrungsdatum`, `sparte`, `umfang`, `geraete` |
/// | 17004 | Kap. 4.13, S. 90–92: `BGM+7`, `IMD++Z13`, `LOC+172`, `SG29 LIN+1` with `IMD++<Grund>` and `DTM+7` | `location`, `grund`, `wertzeitpunkt` |
fn orders_spalte<S, R>(
    mut builder: builders::OrdersBuilder<S, R>,
    pid: Option<u32>,
    p: &mako_engine::payload::OrdersPayload,
    mt: &'static str,
) -> Result<builders::OrdersBuilder<S, R>, RenderError> {
    let missing = |field: &'static str| RenderError::MissingField {
        message_type: mt.into(),
        field: field.into(),
    };
    match pid {
        Some(17_001) => {
            let sparte = payload_sparte(p.sparte, mt, "sparte")?;
            builder = builder
                .document_code("Z10")
                .ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    sparte,
                )?)
                .reference("AAG", required(&p.angebotsnummer, mt, "angebotsnummer")?);
            let (name, kanaele) = kommunikationswege(
                p.ansprechpartner
                    .as_ref()
                    .ok_or_else(|| missing("ansprechpartner"))?,
                mt,
            )?;
            builder = builder.contact(name, kanaele);
            if p.positionen.is_empty() {
                return Err(missing("positionen"));
            }
            for nummer in &p.positionen {
                builder = builder.position(builders::OrdersPosition {
                    referenzen: vec![("Z03".to_owned(), nummer.clone())],
                    ..Default::default()
                });
            }
        }
        Some(17_009) => {
            let sparte = payload_sparte(p.sparte, mt, "sparte")?;
            builder = builder
                .document_code("Z13")
                .ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    sparte,
                )?)
                .abonnement(required(&p.umfang, mt, "umfang")?);
            if p.geraete.is_empty() {
                return Err(missing("geraete"));
            }
            for geraet in &p.geraete {
                let nummern = geraet
                    .geraetenummern
                    .iter()
                    .map(|n| ("Z09".to_owned(), n.clone()))
                    .collect::<Vec<_>>();
                if nummern.is_empty() {
                    return Err(missing("geraete[].geraetenummern"));
                }
                builder = builder.position(builders::OrdersPosition {
                    geraeteart: Some(geraet.geraeteart.clone()),
                    referenzen: nummern,
                    ..Default::default()
                });
            }
        }
        Some(17_004) => {
            builder =
                builder
                    .document_code("7")
                    .abonnement("Z13")
                    .position(builders::OrdersPosition {
                        merkmal: Some(required(&p.grund, mt, "grund")?.to_owned()),
                        gueltig: Some(prozessdatum_303(
                            mt,
                            "wertzeitpunkt",
                            required(&p.wertzeitpunkt, mt, "wertzeitpunkt")?,
                            mako_fristen::Sparte::Strom,
                        )?),
                        ..Default::default()
                    });
        }
        // ORDERS AHB 1.1b Kap. 4.14.2, S. 93–94: `BGM+Z29`, `DTM+203` the day
        // the Rechnungsabwicklung ends (`[UB1]`, Strom), `IMD++Z02` Ende Abo,
        // the Marktlokation in `LOC+172`, and — from the MSB (`[15]`) — one
        // `SG29 LIN+1` with its `IMD` Grund `Z61`/`Z62`/`Z63`. The workflow
        // decides whether the Grund rides; the renderer writes what it holds.
        Some(17_006) => {
            required(&p.location, mt, "location (SG2 LOC+172 Marktlokation)")?;
            builder = builder
                .document_code("Z29")
                .ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    mako_fristen::Sparte::Strom,
                )?)
                .abonnement("Z02");
            if let Some(grund) = text(&p.grund) {
                builder = builder.position(builders::OrdersPosition {
                    merkmal: Some(grund.to_owned()),
                    ..Default::default()
                });
            }
        }
        // ORDERS AHB 1.1b Kap. 4.12, S. 83–87: `BGM+Z12`, `DTM+203`, the
        // Messlokation in `LOC+172` (`[522]`), one `SG29 LIN+Z19` with
        // `PIA+5+<code>:Z11` per Produkt (`[2095]`), and — with the Produkt
        // „weitere Energieflussrichtung" (`[152]`) — `SG3 RFF+Z18` the
        // Marktlokation and `SG2 NAD+DEB` its MSB (`[571]`).
        Some(17_011) => {
            required(&p.location, mt, "location (SG2 LOC+172 Messlokation)")?;
            if p.produkte.is_empty() {
                return Err(missing("produkte (SG29 LIN+Z19 PIA)"));
            }
            builder = builder
                .document_code("Z12")
                .ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    mako_fristen::Sparte::Strom,
                )?);
            if p.produkte
                .iter()
                .any(|c| c == mako_wim::technik_aenderung::PRODUKT_WEITERE_ENERGIEFLUSSRICHTUNG)
            {
                builder = builder
                    .location_reference(
                        "Z18",
                        required(&p.marktlokation, mt, "marktlokation (SG3 RFF+Z18)")?,
                    )
                    .party(builders::OrdersParty {
                        qualifier: "DEB".to_owned(),
                        mp_id: required(&p.msb_marktlokation, mt, "msb_marktlokation (NAD+DEB)")?
                            .to_owned(),
                        referenzen: Vec::new(),
                    });
            }
            for code in &p.produkte {
                builder = builder.position(builders::OrdersPosition {
                    handlung: Some("Z19".to_owned()),
                    produkt: Some(code.clone()),
                    ..Default::default()
                });
            }
        }
        // ORDERS AHB 1.1b Kap. 5.1, S. 116–119: the MaBiS Anforderungen.
        Some(pid @ 17_201..=17_205) => {
            let abo = required(&p.abonnement, mt, "abonnement (IMD Z01/Z02/Z03)")?;
            let zulaessig: &[&str] = match pid {
                17_201 => &["Z01", "Z02"],
                17_202 | 17_203 => &["Z01", "Z02", "Z03"],
                _ => &["Z03"],
            };
            if !zulaessig.contains(&abo) {
                return Err(RenderError::InsufficientPayload {
                    message_type: mt.into(),
                    detail: format!("ORDERS {pid} admits IMD {zulaessig:?}, not {abo:?}").into(),
                });
            }
            builder = builder
                .document_code(match pid {
                    17_201 => "Z19",
                    17_203 => "E40",
                    _ => "Z05",
                })
                .abonnement(abo);
            // `DTM+203` [33]/[34] on 17202/17203 beside Z01/Z02; `DTM+273`
            // `610` beside Z03 ([1]) and always on 17204/17205.
            if matches!(pid, 17_202 | 17_203) && abo != "Z03" {
                builder = builder.ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    mako_fristen::Sparte::Strom,
                )?);
            }
            let ohne_abo = abo == "Z03";
            if ohne_abo {
                builder = builder.zeitintervall(required(
                    &p.zeitintervall,
                    mt,
                    "zeitintervall (CCYYMM)",
                )?);
            }
            // `SG2 LOC+107` Bilanzierungsgebiet on a 17203 ohne Abo.
            if pid == 17_203 && ohne_abo {
                builder = builder.receiver_location(
                    "107",
                    required(&p.bilanzierungsgebiet, mt, "bilanzierungsgebiet (LOC+107)")?,
                );
            }
            let mut position = builders::OrdersPosition::default();
            if pid == 17_201 {
                position.klasse =
                    Some(required(&p.profilgruppe, mt, "profilgruppe (SG30 CCI)")?.to_owned());
            }
            // `SG34 RFF+AUU` Version and `SG38 LOC+172` Meldepunkt on 17202
            // (Version only beside Z03), 17204 and 17205; `SG38 LOC+237` the
            // Bilanzkreis on 17203.
            if matches!(pid, 17_202 | 17_204 | 17_205) {
                if pid != 17_202 || ohne_abo {
                    position.referenzen.push((
                        "AUU".to_owned(),
                        required(&p.version, mt, "version (SG34 RFF+AUU)")?.to_owned(),
                    ));
                }
                position.orte.push((
                    "172".to_owned(),
                    required(&p.meldepunkt, mt, "meldepunkt (SG38 LOC+172)")?.to_owned(),
                ));
            }
            if pid == 17_203 {
                position.orte.push((
                    "237".to_owned(),
                    required(&p.bilanzkreis, mt, "bilanzkreis (SG38 LOC+237)")?.to_owned(),
                ));
            }
            builder = builder.position(position);
        }
        // ORDERS AHB 1.1b Kap. 5.2, S. 120–122 (17206–17208) and Kap. 5.4,
        // S. 125–126 (17210):
        //
        // | PID | `BGM` | `IMD` | `DTM+203` | `DTM+273` / `SG34 RFF+AUU` | `SG38 LOC` |
        // |---|---|---|---|---|---|
        // | 17206 | `Z05` | Z01/Z02/Z03 | beside Z01/Z02 (`[33]`/`[34]`) | beside Z03 (`[1]`) | `172` Zählpunkt der BG-SZR |
        // | 17207 | `BK` | Z01/Z02 | Muss | — | `237` Bilanzkreis |
        // | 17208 | `Z05` | Z03 | — | Muss | `172` Zählpunkt der DZR |
        // | 17210 | `Z05` | Z01/Z02/Z03 | beside Z01/Z02 | beside Z03 | `172` Zählpunkt der LF-AASZR |
        Some(pid @ (17_206 | 17_207 | 17_208 | 17_210)) => {
            let abo = required(&p.abonnement, mt, "abonnement (IMD Z01/Z02/Z03)")?;
            let zulaessig: &[&str] = match pid {
                17_207 => &["Z01", "Z02"],
                17_208 => &["Z03"],
                _ => &["Z01", "Z02", "Z03"],
            };
            if !zulaessig.contains(&abo) {
                return Err(RenderError::InsufficientPayload {
                    message_type: mt.into(),
                    detail: format!("ORDERS {pid} admits IMD {zulaessig:?}, not {abo:?}").into(),
                });
            }
            let ohne_abo = abo == "Z03";
            builder = builder
                .document_code(if pid == 17_207 { "BK" } else { "Z05" })
                .abonnement(abo);
            if !ohne_abo {
                builder = builder.ausfuehrungsdatum(prozessdatum_303(
                    mt,
                    "ausfuehrungsdatum",
                    required(&p.ausfuehrungsdatum, mt, "ausfuehrungsdatum")?,
                    mako_fristen::Sparte::Strom,
                )?);
            } else {
                builder = builder.zeitintervall(required(
                    &p.zeitintervall,
                    mt,
                    "zeitintervall (CCYYMM)",
                )?);
            }
            let mut position = builders::OrdersPosition::default();
            if ohne_abo {
                position.referenzen.push((
                    "AUU".to_owned(),
                    required(&p.version, mt, "version (SG34 RFF+AUU)")?.to_owned(),
                ));
            }
            if pid == 17_207 {
                position.orte.push((
                    "237".to_owned(),
                    required(&p.bilanzkreis, mt, "bilanzkreis (SG38 LOC+237)")?.to_owned(),
                ));
            } else {
                position.orte.push((
                    "172".to_owned(),
                    required(&p.meldepunkt, mt, "meldepunkt (SG38 LOC+172)")?.to_owned(),
                ));
            }
            builder = builder.position(position);
        }
        // ORDERS AHB 1.1b Kap. 5.5, S. 126–127: `BGM+Z58`, `FTX+AFH` the
        // `E_0100` code, `SG1 RFF+ACW` the reclaimed MSCONS' `BGM` (`[525]`).
        Some(17_211) => {
            builder = builder
                .document_code("Z58")
                .reklamation(
                    required(&p.reklamation_code, mt, "reklamation_code (FTX+AFH)")?,
                    "E_0100",
                )
                .reference(
                    "ACW",
                    required(
                        &p.referenz_nachricht,
                        mt,
                        "referenz_nachricht (SG1 RFF+ACW)",
                    )?,
                );
        }
        _ => {}
    }
    Ok(builder)
}

// ── ORDCHG ────────────────────────────────────────────────────────────────────

/// Render an ORDCHG (Purchase Order Change) from domain-intent JSON.
///
/// Used for Stornierung of a pending order — chiefly PID 39000 (Stornierung
/// Sperr-/Entsperrauftrag, LF → NB) emitted by `gpke-sperrung-lf`, and PID 39002
/// (Stornierung der Bestellung, ESA → MSB).
///
/// Payload keys: `pid` (u32, required for the document ID), `sender` (optional —
/// falls back to the registry), `message_ref` (optional — falls back to the
/// causation event ID). `receiver` is always `msg.recipient`.
pub(super) fn render_ordchg(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDCHG";
    let p: mako_engine::payload::OrdchgPayload = super::typed_payload(p)?;

    let pid = checked_pid(p.pid, mt)?;

    let sender = p
        .sender
        .as_deref()
        .unwrap_or_else(|| registry.primary_mp_id());

    let explicit_ref = text(&p.message_ref).map(msg_ref_from_uuid);
    let causation_ref = msg_ref_from_uuid(&msg.causation_event_id.to_string());
    let message_ref = explicit_ref.as_deref().unwrap_or(causation_ref.as_str());

    let release = release_for(msg, MessageType::Ordchg, ReleaseTrack::Short)?;

    let mut builder = builders::OrdchgBuilder::new(release)
        .sender(sender)
        .receiver(msg.recipient.as_ref())
        .message_ref(message_ref);

    if let Some(pv) = pid {
        // `SG1 RFF+Z13`; BGM DE 1004 keeps the Belegnummer.
        builder = builder.pruefidentifikator(pv);
    }
    // ORDCHG mandates an SG1 RFF and carries no LOC. Both references are Muss
    // on the ESA Stornierung (ORDCHG AHB 1.1 Kap. 3.2): `RFF+ON` names the ORDERS
    // being cancelled — its published Zuordnungsschlüssel `ZG-T51`, and the
    // only way the MSB can identify the target — and `RFF+Z13` the PID. They
    // are additive, not alternatives — a Stornierung without `RFF+ON` is one
    // the MSB cannot correlate.
    match text(&p.korrelation_ref).or_else(|| text(&p.order_reference)) {
        Some(reference) => {
            let qualifier = esa_korrelation_qualifier(pid).unwrap_or("ON");
            builder = builder.reference(qualifier, reference);
        }
        None => {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: "order_reference (SG1 RFF+ON — the ORDERS being changed or cancelled)"
                    .into(),
            });
        }
    }
    // `BGM+Z57` — „Übermittlung von Werten an ESA" (the builder already emits
    // DE 1225 = 1, Aufhebung/Stornierung).
    if pid == Some(39002) {
        builder = builder.document_code("Z57");
    }

    finish_interchange(builder.serialize(), sender, msg.recipient.as_ref(), msg)
}

// ── ORDRSP ────────────────────────────────────────────────────────────────────

/// Render an ORDRSP (Purchase Order Response) from domain-intent JSON.
///
/// Used for WiM Stornierung responses (PIDs 39001/39002), WiM Geräteübernahme
/// responses (PIDs 17003/17004), and any other ORDERS-response workflow paths.
///
/// Payload fields:
///
/// | Field          | Required | Description                                   |
/// |----------------|----------|-----------------------------------------------|
/// | `sender`       | no       | Sender MP-ID (falls back to `registry.primary_mp_id()`)|
/// | `receiver`     | no       | Receiver MP-ID (falls back to `msg.recipient`)  |
/// | `document_id`  | no       | BGM document identifier (Auftragsnummer)      |
/// | `document_code`| no       | BGM DE 1001 (`Z14` on 19101)                  |
/// | `order_reference` | no    | `SG1 RFF+ON` — the answered ORDERS' BGM DE 1004 |
/// | `antwort_code` / `antwort_codeliste` | on an Antwortcode-PID | `SG2 AJT` DE 4465 / DE 1082 |
/// | `document_date`| no       | Document date (`YYYYMMDD` or `YYYY-MM-DD`)    |
/// | `ausfuehrungsdatum` + `sparte` | 19001/19002 | `DTM+203` — the German day, begun in `sparte` |
/// | `ansprechpartner` | 19001/19002 | `SG6 CTA+IC` + one `COM` per `kommunikation` entry |
/// | `eigentuemer`  | 19016 with `Z07` | `SG3 NAD+VY` — the Eigentümer's MP-ID   |
/// | `lieferadresse`| 19015 with `ZB5` | `SG3 NAD+Z22` Lieferadresse für Altgeräte |
/// | `message_ref`  | no       | Derived from `causation_event_id` when absent |
///
/// The last four are the WiM Geräteübernahme answers' (ORDRSP AHB 1.1b Kap.
/// 4.9.2 / 4.10); `mako-wim` decides which one an answer carries and refuses
/// a missing one, so the renderer writes what the payload holds.
pub(super) fn render_ordrsp(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "ORDRSP";
    let p: mako_engine::payload::OrdrspPayload = super::typed_payload(p)?;

    let sender = p
        .sender
        .as_deref()
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p.receiver.as_deref().unwrap_or(msg.recipient.as_ref());
    let document_id = text(&p.document_id);
    let pid = checked_pid(p.pid, mt)?;
    let doc_date = text(&p.document_date)
        .map(|d| dokumentdatum_303(mt, d))
        .transpose()?;
    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Ordrsp, ReleaseTrack::Short)?;

    let mut builder = builders::OrdrespBuilder::new(release.clone())
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    // ESA / MaKo: the Prüfidentifikator (BGM DE 1004) is the routing key of the
    // answer — 19011/19012 (Ab-/Bestellung) or 19013/19014 (Stornierung).
    if let Some(pid) = pid {
        builder = builder.pruefidentifikator(pid);
    }
    if let Some(id) = document_id {
        builder = builder.document_id(id);
    }
    if let Some(code) = text(&p.document_code) {
        builder = builder.document_code(code);
    }
    // `SG1 RFF+ON` — the ORDERS this ORDRSP answers, by its `BGM` DE 1004: the
    // Ablehnung der Anfrage Stammdaten 19101 (ORDRSP AHB 1.1b Kap. 4.1.2,
    // Hinweis `[521]`).
    if let Some(order) = text(&p.order_reference) {
        builder = builder.reference("ON", order);
    }
    // Reference the original ORDERS/ORDCHG this ORDRSP answers, under the
    // qualifier the PID's own Zuordnungsschlüssel names: `RFF+ON` for
    // 19011/19012 (the ORDERS, `ZG-T14`), `RFF+ACW` for 19013/19014 (the
    // ORDCHG, `ZG-T50`). These are not interchangeable — they point at
    // different messages.
    if let Some(reference) = text(&p.korrelation_ref) {
        let qualifier = esa_korrelation_qualifier(pid).unwrap_or("ACW");
        builder = builder.reference(qualifier, reference);
    }
    // `SG1 RFF+TN` — the reclaimed Vorgang, after `RFF+ACW` (ORDRSP AHB 1.1b
    // Kap. 4.5, S. 33–35, Nr 00013).
    if let Some(vorgang) = text(&p.referenz_vorgangsnummer) {
        builder = builder.reference("TN", vorgang);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }
    builder = geraeteubernahme_angaben(builder, &p, mt)?;
    // `IMD` DE 7081 where the answer echoes the Anfrage's Leistung — `Z13` on
    // the 19007, `Z11` on the 19114, the 17102's own on a 19102 under `BGM+7`
    // (ORDRSP AHB 1.1b Kap. 4.13, 4.8, 4.1.3).
    if let Some(leistung) = text(&p.leistung) {
        builder = builder.abonnement(leistung);
    }

    // ── `SG2 AJT` — the Antwortcode, on every ORDRSP that carries a decision ──
    //
    // ORDRSP AHB 1.1b Kap. 4 marks `SG2 AJT` **Muss** on all of them: DE 4465
    // the Prüfschritt code, DE 1082 the **Codeliste** it comes from. Conditions
    // [17]/[18] require the code to sit in that list's Zustimmungs- resp.
    // Ablehnungs-Cluster, so it comes from `mako-pruefung` via the workflow and
    // is never synthesised here.
    //
    // DE 1082 is the EBD number only where the AHB says „EBD Nr." — the ESA
    // answers (`E_0254`/`E_0256`/`E_0257`) and the Messlokationsänderung
    // (`E_0249`/`E_0250`) do; every WiM MSB-Wechsel ORDRSP names an `S_00xx`
    // (Strom) or `G_00xx` (Gas) Codeliste instead. The workflow supplies the
    // wire value in `antwort_codeliste`.
    //
    // Which PIDs: 19001/19002 (Bestellung Geräteübernahme), 19003/19004
    // (Weiterverpflichtung), 19005/19006 (Messlokationsänderung), 19009/19010
    // (Beendigung Rechnungsabwicklung), 19011–19014 (ESA Wertebestellung),
    // 19015/19016 (Gerätewechselabsicht), 19101 (Ablehnung der Anfrage
    // Stammdaten, `E_0441` / `E_0443`), 19118/19119 (Antwort auf die Anfrage
    // Sperrung, `E_0488` / `E_1001`), the Datenabruf Ablehnungen 19007 / 19102
    // / 19114 and the GPKE Teil 3 Mitteilungen 19120 / 19127 / 19131 / 19132.
    const ANTWORTCODE_PIDS: &[u32] = &[
        19_001, 19_002, 19_003, 19_004, 19_005, 19_006, 19_009, 19_010, 19_011, 19_012, 19_013,
        19_014, 19_015, 19_016, 19_101, 19_118, 19_119, 19_007, 19_102, 19_114, 19_120, 19_127,
        19_131, 19_132, 19_110, 19_121, 19_124, 19_130,
    ];
    if pid.is_some_and(|p| ANTWORTCODE_PIDS.contains(&p)) {
        // 19011–19014 additionally carry `BGM+Z57` and, on the Bestellung pair,
        // `IMD++<7081>` — which is what tells an answer to a Bestellung
        // (`E_0256`) from one to a Beendigung (`E_0254`), since the two share
        // these Prüfidentifikatoren.
        if matches!(pid, Some(19011..=19014)) {
            builder = builder.document_code("Z57");
            if let Some(abo) = text(&p.abonnement)
                && matches!(pid, Some(19011 | 19012))
            {
                builder = builder.abonnement(abo);
            }
        }
        let code = text(&p.antwort_code);
        let codeliste = text(&p.antwort_codeliste);
        if let (Some(code), Some(codeliste)) = (code, codeliste) {
            // 19013's column marks DE 4465 alone: no Codeliste there.
            let lists_codeliste = super::column_lists(
                MessageType::Ordrsp,
                &release,
                pid.unwrap_or(0),
                "AJT",
                "1082",
            );
            builder = builder.adjustment(code, if lists_codeliste { codeliste } else { "" });
            // `SG2 FTX+AAP` „Antwort (Freier Text)" — the problem the tree
            // requires named, e.g. beside `A02`/`A03` of `E_0488`/`E_1001` on
            // 19119 (ORDRSP AHB 1.1b Kap. 4.3.1, `[33]`/`[43]`).
            if let Some(freitext) = text(&p.antwort_freitext).filter(|t| !t.trim().is_empty()) {
                builder = builder.adjustment_text("AAP", freitext);
            }
        } else {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: format!(
                    "antwort_code/antwort_codeliste (SG2 AJT is Muss on {ANTWORTCODE_PIDS:?})"
                )
                .into(),
            });
        }
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// The WiM Geräteübernahme answers' segments beyond the code: `DTM+203`,
/// `SG6 CTA`/`COM`, `SG3 NAD+VY` and `SG3 NAD+Z22` — each from the payload
/// field `mako-wim` wrote it to (see [`render_ordrsp`]).
fn geraeteubernahme_angaben<S, R>(
    mut builder: builders::OrdrespBuilder<S, R>,
    p: &mako_engine::payload::OrdrspPayload,
    mt: &'static str,
) -> Result<builders::OrdrespBuilder<S, R>, RenderError> {
    if let Some(datum) = text(&p.ausfuehrungsdatum) {
        let sparte = payload_sparte(
            p.sparte,
            mt,
            "sparte (DTM+203 is a day boundary of the recipient's Sparte)",
        )?;
        builder = builder.execution_date(prozessdatum_303(mt, "ausfuehrungsdatum", datum, sparte)?);
    }
    // `DTM+Z02` verschobener Abmeldetermin — Muss on 19003/19004, the day's
    // beginning in the Sparte (`[UB3]`, ORDRSP AHB 1.1b Kap. 4.11, S. 48).
    if let Some(datum) = text(&p.abmeldetermin) {
        let sparte = payload_sparte(
            p.sparte,
            mt,
            "sparte (DTM+Z02 is a day boundary of the recipient's Sparte)",
        )?;
        builder = builder.verschobener_abmeldetermin(prozessdatum_303(
            mt,
            "abmeldetermin",
            datum,
            sparte,
        )?);
    }
    // `DTM+292` geplanter Behebungszeitpunkt — a 19130 with `A06` (ORDRSP AHB
    // 1.1b Kap. 4.7, S. 37); an instant, not a day.
    if let Some(instant) = text(&p.behebungszeitpunkt) {
        builder = builder.behebungszeitpunkt(instant);
    }
    if let Some(kontakt) = &p.ansprechpartner {
        let (name, kanaele) = kommunikationswege(kontakt, mt)?;
        builder = builder.contact(name, kanaele);
    }
    if let Some(mp_id) = text(&p.eigentuemer) {
        builder = builder.involved_party(mp_id);
    }
    if let Some(a) = &p.lieferadresse {
        builder = builder.delivery_address(
            &a.name,
            &a.strasse,
            &a.ort,
            a.postleitzahl.as_deref().unwrap_or(""),
            &a.land,
        );
    }
    Ok(builder)
}

/// Render a QUOTES (Angebot) envelope — the MSB's answer to an ESA Werteanfrage
/// (REQOTE 35003), UC 4.1 Nr. 2. Prüfidentifikator 15003 in BGM DE 1004.
///
/// Payload fields:
///
/// | Field             | Required | Description                                  |
/// |-------------------|----------|----------------------------------------------|
/// | `pid`             | yes      | Prüfidentifikator (15003)                    |
/// | `sender`          | no       | Sender MP-ID (falls back to primary)         |
/// | `receiver`        | no       | Receiver MP-ID (falls back to `msg.recipient`)|
/// | `document_id`     | no       | BGM document id (Angebotsnummer)             |
/// | `korrelation_ref` | no       | `RFF+AAV` — the REQOTE this answers (`ZG-T16`) |
/// | `bindungsfrist_tage` | no    | `DTM+273` — a **duration**, in Tagen          |
/// | `fruehester_start`| no       | `DTM+469` — earliest delivery start           |
/// | `messprodukt`     | no       | `SG27 PIA+5` — the offered Messprodukt        |
/// | `artikel_ids` / `preise` | no | `PIA+Z02` / `SG31 PRI+CAL` per Artikel-ID    |
/// | `document_date`   | no       | DTM+137 date                                 |
/// | `message_ref`     | no       | Derived from `causation_event_id` when absent|
pub(super) fn render_quotes(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    registry: &MpIdRegistry,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "QUOTES";
    let p: mako_engine::payload::QuotesPayload = super::typed_payload(p)?;

    let sender = p
        .sender
        .as_deref()
        .unwrap_or_else(|| registry.primary_mp_id());
    let receiver = p.receiver.as_deref().unwrap_or(msg.recipient.as_ref());
    let doc_date = text(&p.document_date)
        .map(|d| dokumentdatum_303(mt, d))
        .transpose()?;
    let message_ref = text(&p.message_ref)
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let release = release_for(msg, MessageType::Quotes, ReleaseTrack::Short)?;

    let mut builder = builders::QuotesBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .message_ref(message_ref);

    let pid = checked_pid(p.pid, mt)?;
    if let Some(pid) = pid {
        builder = builder.pruefidentifikator(pid);
    }
    if let Some(id) = text(&p.document_id) {
        builder = builder.document_id(id);
    }
    if let Some(d) = doc_date.as_deref() {
        builder = builder.document_date(d);
    }
    // Echo the location so an ESA can correlate this answer to its process.
    if let Some(loc) = text(&p.location) {
        builder = builder.location(loc);
    }
    // Bindungsfrist (Angebot only) — its presence distinguishes an Angebot from
    // an Anfrage-Ablehnung on the ESA inbound side.
    //
    // `DTM+273` is a **duration**: DE 2380 „Zeitraum" (1 bis n) plus DE 2379
    // ∈ {802 Monat, 803 Woche, 804 Tag}. The workflow hands over a day count.
    if let Some(tage) = p.bindungsfrist_tage
        && tage > 0
    {
        builder = builder.bindungsfrist(tage.to_string(), builders::DauerEinheit::Tag);
    }
    // Ablehnungsgrund (Ablehnung der Anfrage) — free text.
    if let Some(reason) = text(&p.reason) {
        builder = builder.reason(reason);
    }

    // ── ESA Angebot (PID 15003) full-conformance content ─────────────────────
    // 15003 mandates SG1 RFF, SG4 CUX, SG14 CTA/COM, SG27 LIN/PIA, SG31 PRI on
    // top of the shared skeleton. Emit them (payload-supplied, sensible
    // defaults) only for 15003, so the Geräteübernahme Angebote (15001/15002)
    // that share this renderer stay untouched. An Ablehnung (reason set) carries
    // no Angebot content.
    let is_angebot = pid == Some(15003) && p.reason.is_none();
    if is_angebot {
        // `BGM+Z57`, `SG1 RFF+AAV` (the REQOTE this answers — `ZG-T16`) and
        // `RFF+Z13`, `SG4 CUX`, `SG14 CTA/COM`, `SG11 NAD+DP` before the
        // `LOC+172`, and the SG27 Messprodukt line with its SG31 prices.
        builder = builder.document_code("Z57").delivery_party();
        if let Some(reference) = text(&p.korrelation_ref) {
            builder = builder.reference("AAV", reference);
        }
        builder = builder.currency(text(&p.currency).unwrap_or("EUR"));
        // `DTM+469` — the earliest start the MSB can deliver from (Muss).
        if let Some(start) = text(&p.fruehester_start) {
            builder = builder.fruehester_start(esa_dtm303(mt, "fruehester_start", start)?);
        }
        let contact = text(&p.contact).unwrap_or("MSB-Service");
        let comm = text(&p.contact_comm).unwrap_or(sender);
        builder = builder.contact(contact, comm);
        // The Messprodukt is the one that was asked for — the Angebot prices
        // *that* product (condition [77]), so it is echoed rather than
        // defaulted to an OBIS-Kennzahl, which is a different PIA qualifier.
        if let Some(messprodukt) = text(&p.messprodukt) {
            builder = builder.product(messprodukt);
        }
        for id in &p.artikel_ids {
            builder = builder.artikel_id(id);
        }
        for obis in &p.obis {
            builder = builder.obis_kennzahl(obis);
        }
        // `PRI+CAL` per Artikel-ID: Z01 Einrichtungs-, Z02 Transaktions-,
        // Z03 Betriebspreis, with H87 Stück / DAY Tag as the base unit.
        for preis in &p.preise {
            let betrag = preis.betrag.as_str();
            let art = preis.art.as_deref().unwrap_or("Z01");
            let einheit =
                preis
                    .einheit
                    .as_deref()
                    .unwrap_or(if art == "Z03" { "DAY" } else { "H87" });
            builder = builder.preis(betrag, art, einheit);
        }
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}
