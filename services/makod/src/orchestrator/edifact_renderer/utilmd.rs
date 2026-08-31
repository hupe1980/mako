//! UTILMD renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use super::*;
// ── UTILMD ────────────────────────────────────────────────────────────────────

/// Render a UTILMD outbound message from domain-intent JSON.
///
/// Payload fields (all sourced from workflow `handle` implementations):
///
/// | Field           | Required | Description                                  |
/// |-----------------|----------|----------------------------------------------|
/// | `pid`           | yes      | Prüfidentifikator (u32)                       |
/// | `sender`        | yes      | Sender MP-ID (our own)                        |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`) |
/// | `malo` / `melo` | yes*     | Lokations-ID → `SG5 LOC+Z16` / `LOC+Z17`      |
/// | `vorgangsnummer`| no       | `IDE+24` DE 7402 (defaults to the message ref) |
/// | `referenz_vorgangsnummer` | on answers | `SG4 SG6 RFF+TN` — the **request's** `IDE+24` |
/// | `process_date`  | yes      | Process date (`YYYYMMDD` or `YYYY-MM-DD`)     |
/// | `document_date` | no       | Document date (defaults to today at dispatch time) |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent  |
/// | `transaktionsgrund` | no   | `SG4 STS+7` DE 9013 element 2                  |
/// | `transaktionsgrund_ergaenzung` | no | `STS+7` DE 9013 element 3 (`ZW3`…`ZAP`); defaults to `ZW4` when a Grund is present |
/// | `antwort_code`  | no       | `SG4 STS+E01` DE 9013 — **required on every Antwort-PID** |
/// | `antwort_codeliste` | no   | `STS+E01` DE 1131, the **Codeliste** the code comes from (`E_0622`, `S_0090`, `G_0051`, …) |
/// | `bemerkung`     | no       | `FTX+ACB` free text (mandatory alongside a catch-all Ablehnungscode) |
/// | `bilanzkreis`   | on 55001/55014/55608, 44001 | Strom: `SG8 SEQ+Z79` Produktpaket · Gas: `SG10 CCI+Z19` — its own slot, never `bemerkung` |
/// | `document_code` | no       | `BGM` DE 1001, when the Anwendungsfall fixes one other than `E01` |
/// | `lokationstyp`  | no       | `SG5 LOC` DE 3227 — `Z21` Tranche, Gas `172` Meldepunkt; defaults to the PID's own object |
/// | `beteiligte_marktpartner` | on 55036/55038, 44036/44038 | `SG12 NAD+VY` — every Altlieferant, or the auslösender Marktpartner |
/// | `kunde_name` | on 55010 (`ZW4`/`ZAP`) | `SG12 NAD+Z09` „Kunde des LF" — a **name**, in `C080`, not an MP-ID |
/// | `kunde_namensformat` | with `kunde_name` | `NAD` DE 3045 — `Z01` Person, `Z02` Firma; defaults to `Z01` |
/// | `dritter_antwortcode` | **on `A50` / `A57`** | `SG4 STS+Z35` — the LFA's own `E_0624` code, restated |
/// | `dritter_referenz_lokation` | erzeugende Ablehnung | `STS+Z35` `C555` DE 9012 — which MaLo/Tranche the restated answer is about |
/// | `dritter_objekt` | erzeugende Ablehnung | the second DE 9013 — `ZW3` Erzeugende MaLo / `ZW5` Tranche |
/// | `bilanzierungsbeginn` | 55238/55239 | second `SG4 DTM+158`, Muss on the Modell-2 Anmeldung |
/// | `bilanzierungsende` | Gas 44037/44038, 55240–55243 | second `SG4 DTM+159`, Soll „wenn eine Bilanzierung stattfindet" |
/// | `mabis_zaehlpunkt` | 55239 | second `SG5 LOC+Z15` — the ZPB des ZP der NGZ, beside the MaLo |
/// | `dtm_qualifier`  | 55611     | overrides the per-PID `SG4 DTM` DE 2005 — there it follows the Grund |
///
/// \* Exactly one of `malo` / `melo` / `mabis_zaehlpunkt` is required. The first
/// two follow the PID range; a MaBiS Vorgang names only the third, and it then
/// rides the primary `SG5 LOC+Z15` rather than a second one.
///
/// # What the MIG fixes here
///
/// `IDE` DE 7495 has exactly two values (`24` Vorgang, `Z01` Liste) and DE 7402
/// carries a **Vorgangsnummer** — the Lokations-ID belongs in `SG5 LOC`. The SG4
/// date qualifiers are `92`/`93`/`157`/`76`, never the Messperioden-Qualifier
/// `163`/`164`.
///
/// The Prüfidentifikator travels in `SG4 SG6 RFF+Z13`, „genau einmal je SG4 IDE
/// (Vorgang) anzugeben"; the builder emits it. An answer additionally carries
/// `SG4 SG6 RFF+TN` with the request's Vorgangsnummer, because DE 7402 must be
/// globally unique and so the answer cannot reuse the requester's.
pub(super) fn render_utilmd(
    p: &serde_json::Value,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    use edi_energy::utilmd_codes::{AntwortStatus, Transaktionsgrund, ergaenzung};

    let mt = "UTILMD";

    let pid = require_u32(p, mt, "pid")?;

    // A **Listennachricht** is a different message shape: an `IDE+Z01` head
    // that is the Geschäftsvorfall, with one `IDE+24` member per disputed
    // Marktlokation under it. Rendered by its own path below.
    if let Some(positionen) = p.get("positionen").and_then(|v| v.as_array())
        && !positionen.is_empty()
    {
        return render_utilmd_liste(p, msg, pid, positionen);
    }

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());

    // The MSB is assigned to the **Messlokation**, never to the Marktlokation
    // („Der MSB ist ausschließlich dem Objekt Messlokation zugeordnet" — WiM
    // Strom Teil 1 Kap. 2.1.2 d, AWH WiM Gas 2.0 Kap. 3.1.2 d), so every WiM
    // MSB-Wechsel message names a MeLo in `SG5 LOC` — the Anfrage and both
    // answers, in both Sparten. Everything else names a MaLo.
    let names_messlokation = mako_wim::geraetewechsel::wim_sparte(pid).is_some()
        || mako_wim::antwort_pid_meaning(pid)
            .is_some_and(|(request, _)| mako_wim::geraetewechsel::wim_sparte(request).is_some());
    let location_id_key = if names_messlokation { "melo" } else { "malo" };
    // A MaBiS Vorgang is about a **MaBiS-Zählpunkt** and names no Marktlokation
    // at all — the ZP-Lifecycle Anfragen and their answers carry only
    // `mabis_zaehlpunkt`. Requiring a MaLo there refuses the whole message, and
    // a UTILMD that does not render is raw JSON on the AS4 leg.
    let location_id = match p.get(location_id_key).and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => Some(id),
        _ => match p.get("mabis_zaehlpunkt").and_then(|v| v.as_str()) {
            Some(zp) if !zp.is_empty() => Some(zp),
            _ if utilmd_carries_sg5_loc(pid) => {
                return Err(RenderError::MissingField {
                    message_type: mt.into(),
                    field: format!("{location_id_key} or mabis_zaehlpunkt").into(),
                });
            }
            _ => None,
        },
    };
    // Which `SG5 LOC` DE 3227 the primary Lokation rides. A payload that named
    // only a MaBiS-Zählpunkt has already put it here, so the second-LOC block
    // below must not repeat it.
    let primary_is_mabis_zp = location_id.is_some()
        && p.get(location_id_key)
            .and_then(|v| v.as_str())
            .is_none_or(str::is_empty);

    // `SG4 DTM` is not universal. UTILMD AHB Strom Kap. 8.11 / Gas Kap. 5.8
    // leave both the „Beginn zum" and „Ende zum" columns empty for the
    // Information über existierende Zuordnung (55036 / 44036): it names the LFA
    // and the Vorgang it refers to, and no date of its own. Emitting one there
    // is an unlisted segment, so the field is required everywhere else and
    // refused here.
    let carries_process_date = utilmd_carries_sg4_date(pid);
    let process_date = if carries_process_date {
        Some(require_str(p, mt, "process_date")?)
    } else {
        None
    };

    let doc_date_owned = p
        .get("document_date")
        .and_then(|v| v.as_str())
        .map(normalise_date);
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    // Determine UTILMD release track from PID: 44xxx = Gas, everything else = Strom.
    let track = if (44_000..=44_999).contains(&pid) {
        ReleaseTrack::Gas
    } else {
        ReleaseTrack::Strom
    };
    let release =
        active_release(MessageType::Utilmd, track).ok_or_else(|| RenderError::NoActiveProfile {
            message_type: mt.into(),
        })?;

    let edifact_pid = Pruefidentifikator::new(pid).map_err(|e| RenderError::MissingField {
        message_type: mt.into(),
        field: format!("pid value {pid} is invalid: {e}").into(),
    })?;

    // The `SG4 DTM` DE 2005 qualifier is per-PID for almost everything, and the
    // table below is that mapping. A 55611 is the exception: „Beginn zum" under
    // `STS+7++ZH1` and „Ende zum" under `ZC8` (UTILMD AHB Strom Kap. 8.11
    // Bedingungen `[475]` / `[474]`), so the qualifier follows the **Grund** and
    // only the workflow knows it. An explicit value therefore wins.
    let dtm_qualifier = p
        .get("dtm_qualifier")
        .and_then(|v| v.as_str())
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| utilmd_dtm_qualifier(pid));
    let process_date_yyyymmdd = process_date.map(normalise_date);

    // `SG4 SG6 RFF+Z13` carries the Prüfidentifikator and the builder emits it
    // per Vorgang from `pruefidentifikator` — DE 1154 is `R n5`, so nothing but
    // the five-digit code belongs there.
    let mut builder = builders::UtilmdBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .pruefidentifikator(edifact_pid)
        .message_ref(message_ref.clone());

    // `BGM` DE 1001. `E01` „Anmeldungen" is the default and the right code for
    // most Anwendungsfälle; the ones that end or cancel an assignment are `E02`
    // Abmeldungen, and every Gas Informationsmeldung is `E44`. The workflow that
    // knows its own Anwendungsfall supplies it.
    //
    // The MaBiS-ZP lifecycle is not an Anmeldung in any sense — UTILMD AHB
    // Strom 2.2 Kap. 13.3 fixes `Z07` „Aktivierung/Deaktivierung von MaBiS-ZP"
    // on all three of its Prüfidentifikatoren — so it defaults per PID rather
    // than relying on every caller to remember.
    let document_code = p
        .get("document_code")
        .and_then(|v| v.as_str())
        .or(match pid {
            55_062..=55_064 => Some(edi_energy::utilmd_codes::BGM_MABIS_ZP_LIFECYCLE),
            _ => None,
        });
    if let Some(code) = document_code {
        builder = builder.document_code(code);
    }

    if let Some(dd) = doc_date_owned.as_deref() {
        builder = builder.document_date(dd);
    }

    // `IDE+24` DE 7402. The workflow may supply its own Vorgangsnummer; the
    // message reference is a serviceable default because it is already unique
    // per outbound message and is what the counterparty echoes in RFF.
    let vorgangsnummer = p
        .get("vorgangsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or(message_ref.as_str());

    let mut tx = builder.transaction(vorgangsnummer);
    if let Some(date) = process_date_yyyymmdd.as_deref() {
        tx = tx.date(dtm_qualifier, date);
    }

    // The **second** `SG4 DTM`. Both are dates beside the process date, never a
    // replacement for it, and each has its own named field so a payload cannot
    // state a Vertragsbeginn where the AHB wants a Bilanzierungsbeginn.
    //
    // | Field | DE 2005 | Where |
    // |---|---|---|
    // | `bilanzierungsbeginn` | `158` | UTILMD 55238/55239 — AHB Strom 2.2 Kap. 11, Bedingung `[317]` makes it carry the same value as `DTM+92` |
    // | `bilanzierungsende` | `159` | Gas 44037/44038 „wenn eine Bilanzierung stattfindet" (AHB Gas Kap. 5.8 Bedingung `[29]`); UTILMD 55240–55243 beside `DTM+93` |
    for (field, qualifier) in [
        (
            "bilanzierungsbeginn",
            edi_energy::utilmd_codes::dtm::BILANZIERUNGSBEGINN,
        ),
        (
            "bilanzierungsende",
            edi_energy::utilmd_codes::dtm::BILANZIERUNGSENDE,
        ),
    ] {
        if let Some(date) = p
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            tx = tx.date(qualifier, normalise_date(date));
        }
    }

    // `SG4 SG6 RFF+TN` — „Referenz Vorgangsnummer (aus Anfragenachricht)",
    // Muss on every Antwortnachricht (UTILMD AHB Strom 2.2 / Gas 1.2). The
    // answer's own `IDE+24` must be a fresh number, so this is the only thing
    // that ties it to the request.
    if let Some(referenz) = p
        .get("referenz_vorgangsnummer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        tx = tx.referenz_vorgangsnummer(referenz);
    }

    // `SG4 STS+7` — Transaktionsgrund, and in the GPKE/GeLi Gas processes its
    // Ergänzung. The AHB marks the Ergänzung Muss wherever the Grund is there,
    // and `ZW4` (verbrauchende Marktlokation) is the case every core process
    // describes unless the workflow says otherwise.
    //
    // **The WiM MSB-Wechsel has no Ergänzung.** Its Anwendungsübersichten list
    // `SG4 STS 9015 = 7` and `SG4 STS 9013 = E01|E02|E03|…` and nothing else
    // (UTILMD AHB Strom 2.2 Kap. 10, Gas 1.2 Kap. 6). Defaulting `ZW4` onto a
    // Messlokations-Vorgang states „verbrauchende Marktlokation" in an element
    // the Anwendungsfall does not define.
    //
    // **GeLi Gas has no Ergänzung either.** `ZW3`/`ZW4`/`ZW5`/`ZAP` appear
    // nowhere in UTILMD AHB Gas G1.1/G1.2 — every Gas Anwendungsfall lists a
    // single `SG4 STS 9013` row. Defaulting `ZW4` onto a Gas Vorgang writes a
    // code the receiving AHB does not define into an element it does not use.
    if let Some(grund) = p.get("transaktionsgrund").and_then(|v| v.as_str()) {
        let t = if names_messlokation || track == ReleaseTrack::Gas {
            Transaktionsgrund::bare(grund)
        } else {
            let erg = p
                .get("transaktionsgrund_ergaenzung")
                .and_then(|v| v.as_str())
                .unwrap_or(ergaenzung::VERBRAUCHENDE_MALO);
            Transaktionsgrund::new(grund, erg)
        };
        tx = tx.transaktionsgrund(t);
    }

    // `SG4 STS+E01` — the Prüfschritt code and the Codeliste it comes from.
    // Without it a Bestätigung or Ablehnung is not a well-formed answer: the
    // AHB marks the segment Muss and constrains the code to that Codeliste's
    // cluster. DE 1131 is the Codeliste identifier, which is the EBD number
    // only where the AHB says „EBD Nr." — every WiM MSB-Wechsel answer names an
    // `S_00xx` or `G_00xx` list instead.
    if let Some(code) = p.get("antwort_code").and_then(|v| v.as_str()) {
        let antwort = match p.get("antwort_codeliste").and_then(|v| v.as_str()) {
            Some(cl) => AntwortStatus::from_codeliste(code, cl),
            None => AntwortStatus::bare(code),
        };
        tx = tx.antwort(antwort);

        // `SG4 STS+Z35` — „Status der Antwort des dritten Marktbeteiligten".
        //
        // **Muss when the Antwortcode is `A50` or `A57`** (UTILMD AHB Strom
        // Bedingungen `[356]` / `[84]`): both mean „der LFA hat der Anfrage zur
        // Beendigung der Zuordnung widersprochen", and GPKE Teil 2 § 2.1.2 Nr. 6
        // requires the NB to state that refusal's ground alongside its own.
        // Refusing to render is the only way this stays true — an Ablehnung
        // that omits it tells the LFN its Anmeldung failed and not why the
        // incumbent would not release the Marktlokation, which is the one fact
        // it can act on.
        let dritter = p.get("dritter_antwortcode").and_then(|v| v.as_str());
        if edi_energy::utilmd_codes::CODES_REQUIRING_DRITTER.contains(&code) && dritter.is_none() {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: format!(
                    "antwort_code {code} requires \"dritter_antwortcode\" — UTILMD AHB Strom                      marks SG4 STS+Z35 Muss alongside it, and GPKE Teil 2 § 2.1.2 Nr. 6 has the                      NB state the LFA's own Ablehnungsgrund"
                )
                .into(),
            });
        }
        if let Some(dritter_code) = dritter {
            let referenz = p
                .get("dritter_referenz_lokation")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let objekt = p
                .get("dritter_objekt")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            tx = tx.antwort_dritter(match (referenz, objekt) {
                (Some(r), Some(o)) => {
                    edi_energy::utilmd_codes::DritterAntwortStatus::erzeugend(dritter_code, r, o)
                }
                // The verbrauchende form: a 55003's AHB column carries neither,
                // because a verbrauchende Marktlokation has exactly one LFA and
                // the Vorgang already names it.
                _ => edi_energy::utilmd_codes::DritterAntwortStatus::verbrauchend(dritter_code),
            });
        }
    }

    // `FTX+ACB` Bemerkung — mandatory alongside the catch-all Ablehnungscodes
    // (`A99` Strom, `E14` Gas), which require a written Erläuterung.
    if let Some(text) = p.get("bemerkung").and_then(|v| v.as_str()) {
        tx = tx.free_text("ACB", text);
    }

    // The Bilanzkreis — **two different segments, one per Festlegung**.
    //
    // GPKE Strom carries it in the Produktpaket `SG8 SEQ+Z79` with Produkt-Code
    // `9991000002082` and the value in `SG10 CAV+ZV4` (UTILMD AHB Strom 2.2
    // Kap. 5.3, Codeliste der Konfigurationen 1.4 Kap. 6.1.1), Muss on 55001,
    // 55077, 55600, 55601, 55014 and 55608. GeLi Gas has no Produktpaket at
    // all: UTILMD AHB Gas 1.2 puts the Bilanzkreis in `SG10 CCI+Z19` DE 7037,
    // Muss on 44001. Sending either shape on the other Sparte is a segment the
    // receiving AHB does not define.
    //
    // Neither is an `FTX+ACB` remark: on a Strom Zuordnungs-Bestätigung that
    // segment is admitted on the Ablehnung only (Bedingung [48]).
    if let Some(bilanzkreis) = p
        .get("bilanzkreis")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        tx = match track {
            ReleaseTrack::Gas => tx.merkmal(
                edi_energy::utilmd_codes::produkt::CCI_BILANZKREIS_GAS,
                bilanzkreis,
            ),
            _ => tx.produktpaket(edi_energy::utilmd_codes::Produktpaket::bilanzkreis(
                bilanzkreis,
            )),
        };
    }

    // `SG12 NAD+Z09` „Kunde des LF" — a **name**, so it rides `C080` and not
    // the party-identification composite. Muss on a 55010 whose
    // Transaktionsgrundergänzung is `ZW4`/`ZAP` (Bedingung [279]); it is the
    // „Kundenname aus Anmeldung Lieferant neu" ([572]) the LFA compares against
    // its own contract holder at `E_0624` Prüfschritt 30.
    if let Some(name) = p
        .get("kunde_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        let format = p
            .get("kunde_namensformat")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(edi_energy::utilmd_codes::namensformat::PERSON);
        // A `Z01` Personenname splits across the five DE 3036 components as
        // Nachname, Vorname, …; a `Z02` Firmenbezeichnung is one line. The
        // separator is the caller's, so an unsplit name still renders.
        let parts: Vec<String> = if format == edi_energy::utilmd_codes::namensformat::PERSON {
            name.splitn(5, ',').map(|s| s.trim().to_owned()).collect()
        } else {
            vec![name.to_owned()]
        };
        tx = tx.kunde_des_lf(parts, format);
    }

    // `SG12 NAD+VY` — the beteiligte Marktpartner a Vorgang names beside sender
    // and receiver: every Altlieferant on a 55036/44036 (Bedingung [518]), the
    // auslösender Marktpartner on a 55038/44038 ([579]/[571]).
    if let Some(parties) = p.get("beteiligte_marktpartner").and_then(|v| v.as_array()) {
        for party in parties.iter().filter_map(|v| v.as_str()) {
            tx = tx.beteiligter_marktpartner(party);
        }
    }

    // `SG5 LOC` DE 3227. The qualifier is a property of the *object* the Vorgang
    // is about, which the PID alone does not always fix: a GPKE Vorgang may name
    // a Tranche (`Z21`) instead of a Marktlokation, and every Gas Vorgang names
    // a Meldepunkt (`172`). Both still carry a MaLo-ID in DE 3225.
    //
    // On the **Gas** track the qualifier is `172` Meldepunkt for every
    // Anwendungsfall — UTILMD AHB Gas G1.1/G1.2 defines `Z16` and `Z17`
    // nowhere, and tells a Marktlokation from a Messlokation by the format of
    // DE 3225 instead (`[950]` Marktlokations-ID, `[951]`
    // Zählpunktbezeichnung). Sending the Strom qualifier there is a segment the
    // receiver's own profile rejects, which is why this follows the track and
    // not the caller.
    let tx = match (location_id, p.get("lokationstyp").and_then(|v| v.as_str())) {
        (None, _) => tx,
        (Some(id), Some(qualifier)) if !qualifier.is_empty() => tx.location(
            edi_energy::Lokationstyp::from_qualifier_code(qualifier).ok_or_else(|| {
                RenderError::MissingField {
                    message_type: mt.into(),
                    field: format!("lokationstyp {qualifier:?} is not a LOC DE 3227 qualifier")
                        .into(),
                }
            })?,
            id,
        ),
        (Some(id), _) if track == ReleaseTrack::Gas => {
            tx.location(edi_energy::Lokationstyp::Meldepunkt, id)
        }
        (Some(id), _) if primary_is_mabis_zp => {
            tx.location(edi_energy::Lokationstyp::MabisZaehlpunkt, id)
        }
        (Some(id), _) if names_messlokation => tx.messlokation(id),
        (Some(id), _) => tx.marktlokation(id),
    };

    // A **second** `SG5 LOC`, `Z15` MaBiS-Zählpunkt, beside the Marktlokation.
    // UTILMD AHB Strom 2.2 Bedingung `[663]` makes the Modell-2 Bestätigung
    // (55239) carry „die ID der Marktlokation und die ZPB des ZP der NGZ": the
    // LPB has no other way to learn the Zählpunkt whose Netzgangzeitreihe it
    // just won the right to receive.
    let tx = match p
        .get("mabis_zaehlpunkt")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && !primary_is_mabis_zp)
    {
        Some(zp) => tx.location(edi_energy::Lokationstyp::MabisZaehlpunkt, zp),
        None => tx,
    };

    finish_interchange(tx.done().serialize(), sender, receiver, msg)
}

/// Whether this PID's Anwendungsfall carries an `SG4 DTM` process date at all.
///
/// Almost every UTILMD Vorgang does, so this answers `true` by default and
/// names the exceptions. The Information über existierende Zuordnung
/// (55036 Strom, 44036 Gas) is one: UTILMD AHB Strom Kap. 8.11 and Gas Kap. 5.8
/// leave both its „Beginn zum" and „Ende zum" columns empty. It states who the
/// Altlieferant is and which Anmeldung it refers to — `SG12 NAD+VY` and
/// `SG6 RFF+TN` — and no date of its own.
///
/// The GeLi Gas Stornierung family (44022/44023/44024) is another: it cancels a
/// Vorgang by reference and restates none of its dates. Its AHB table lists
/// `DTM+137` alone — the document date, which the envelope carries — and no
/// „Beginn zum" or „Ende zum" column at all. The MaBiS Korrekturlisten
/// (55066/55196/55202/55224) are the same: they answer a list that covers a
/// Bilanzierungsmonat, not a Vorgang that starts or ends on a day. The
/// MaBiS-ZP lifecycle (55062–55064) states a `DTM+158` Bilanzierungsbeginn on
/// an Aktivierung and a `DTM+159` Bilanzierungsende on a Deaktivierung, and
/// no Vertragsdatum at all — the Zählpunkt has no contract.
///
/// Rendering one anyway emits a segment the receiving AHB does not define for
/// the Anwendungsfall, which is a rejection the sender cannot see coming.
/// Render a **Listennachricht** — the MaBiS Clearinglisten and their answers.
///
/// A different message shape from every other UTILMD: the `SG4 IDE+Z01` head
/// *is* the Geschäftsvorfall and each `IDE+24` under it is a member rather than
/// a Vorgang of its own („Das IDE+Z01 (Liste) definiert den Geschäftsvorfall.
/// Alle aufgelisteten IDE+24 sind Bestandteil des Geschäftsvorfalls" — UTILMD
/// AHB Strom 2.2 Kap. 13.4, Bedingung `[564]`).
///
/// | Where | Segment | Source |
/// |---|---|---|
/// | message | `BGM+Z05` Clearingliste, `DTM+157` Bilanzierungsmonat in `610` `CCYYMM` | Kap. 13.4 |
/// | head | `IDE+Z01` Listennummer, `SG5 LOC+Z15` MaBiS-Zählpunkt (Muss), `SG6 RFF+Z13`, `SG6 RFF+TN` the answered list's number, `SG8 SEQ+Z22` + `RFF+AUU` Version der Zeitreihe (Muss) | Kap. 13.4 |
/// | member | `IDE+24` Vorgangsnummer, `SG4 STS+E01` code + EBD (Muss), `SG5 LOC+Z16` Marktlokation (Muss) | Kap. 13.4 |
///
/// The head's own `STS+E01` and its members are **mutually exclusive**:
/// Bedingung `[238]` marks the head status Muss „wenn SG4 IDE+24 (Vorgang)
/// nicht vorhanden", and `[630]` says „wenn die Liste abgelehnt wird, ist kein
/// Vorgang enthalten". A whole-list Ablehnung therefore takes the ordinary
/// single-Vorgang path above, where its payload carries no `positionen`.
fn render_utilmd_liste(
    p: &serde_json::Value,
    msg: &OutboxMessage,
    pid: u32,
    positionen: &[serde_json::Value],
) -> Result<RenderedInterchange, RenderError> {
    let mt = "UTILMD";

    let sender = require_str(p, mt, "sender")?;
    let receiver = p
        .get("receiver")
        .and_then(|v| v.as_str())
        .unwrap_or(msg.recipient.as_ref());
    let zaehlpunkt = require_str(p, mt, "mabis_zaehlpunkt")?;
    let version = require_str(p, mt, "zeitreihen_version")?;
    let listennummer = require_str(p, mt, "listennummer")?;
    let codeliste = require_str(p, mt, "antwort_codeliste")?;

    let release = active_release(MessageType::Utilmd, ReleaseTrack::Strom).ok_or_else(|| {
        RenderError::NoActiveProfile {
            message_type: mt.into(),
        }
    })?;
    let edifact_pid = Pruefidentifikator::new(pid).map_err(|e| RenderError::MissingField {
        message_type: mt.into(),
        field: format!("pid value {pid} is invalid: {e}").into(),
    })?;
    let message_ref = p
        .get("message_ref")
        .and_then(|v| v.as_str())
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let mut builder = builders::UtilmdBuilder::new(release)
        .sender(sender)
        .receiver(receiver)
        .pruefidentifikator(edifact_pid)
        .message_ref(message_ref)
        // `BGM+Z05` — every Clearingliste and every answer to one.
        .document_code(edi_energy::utilmd_codes::BGM_CLEARINGLISTE);
    if let Some(dd) = p.get("document_date").and_then(|v| v.as_str()) {
        builder = builder.document_date(normalise_date(dd));
    }
    // `DTM+157` in `610` — the Bilanzierungsmonat, `CCYYMM`.
    if let Some(monat) = p
        .get("billing_period")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        builder = builder.gueltigkeit_beginn(monat);
    }

    // ── The head ─────────────────────────────────────────────────────────────
    let mut head = builder
        .list_transaction(listennummer)
        .location(edi_energy::Lokationstyp::MabisZaehlpunkt, zaehlpunkt)
        .summenzeitreihe_version(version);
    // `SG6 RFF+TN` — „Es ist die Listennummer aus der Lieferanten- bzw.
    // Bilanzierungsgebietsclearingliste zu verwenden" (Bedingung `[631]`).
    if let Some(referenz) = p
        .get("referenz_listennummer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        head = head.referenz_vorgangsnummer(referenz);
    }
    let mut builder = head.done();

    // ── The members ──────────────────────────────────────────────────────────
    for (i, pos) in positionen.iter().enumerate() {
        let malo =
            pos.get("malo")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RenderError::MissingField {
                    message_type: mt.into(),
                    field: format!("positionen[{i}].malo").into(),
                })?;
        let code = pos
            .get("antwort_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: format!("positionen[{i}].antwort_code").into(),
            })?;
        // `IDE+24` DE 7402 must be unique across every Vorgangsnummer ever
        // sent, so it is minted from the list number and the position rather
        // than reusing the Marktlokations-ID.
        let vorgangsnummer = pos
            .get("vorgangsnummer")
            .and_then(|v| v.as_str())
            .map_or_else(|| format!("{listennummer}-{}", i + 1), ToOwned::to_owned);
        let mut member = builder
            .transaction(vorgangsnummer)
            .antwort(edi_energy::utilmd_codes::AntwortStatus::from_codeliste(
                code, codeliste,
            ))
            .marktlokation(malo);
        if let Some(referenz) = pos
            .get("referenz_vorgangsnummer")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            member = member.referenz_vorgangsnummer(referenz);
        }
        builder = member.done();
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// Whether this PID's Anwendungsfall names a Lokation at all.
///
/// Almost every UTILMD Vorgang is about one, so this answers `true` by default
/// and names the exceptions. The **MaBiS Korrekturlisten** (55066, 55196,
/// 55202, 55224) are the ones that are not: they answer a whole Clearingliste,
/// and their Ablehnungs-Cluster refuses it outright — the Abonnement was never
/// ordered, the version is not admitted, the Zeitraum is implausible. There is
/// no Marktlokation such an answer is about, and the AHB marks `SG5 LOC` Kann
/// on the family accordingly.
///
/// Requiring one there refuses the whole message, and a UTILMD that does not
/// render leaves raw JSON on the AS4 leg.
pub(super) const fn utilmd_carries_sg5_loc(pid: u32) -> bool {
    !matches!(pid, 55_066 | 55_196 | 55_202 | 55_224)
}

pub(super) const fn utilmd_carries_sg4_date(pid: u32) -> bool {
    !matches!(
        pid,
        55_036
            | 44_036
            | 44_022..=44_024
            | 55_062..=55_064
            | 55_066
            | 55_196
            | 55_202
            | 55_224
    )
}

/// The `SG4 DTM` DE 2005 qualifier for the process date of a given PID.
///
/// | Process | Qualifier | MIG name |
/// |---|---|---|
/// | Anmeldung / Lieferbeginn | `92` | Beginn zum (Datum Vertragsbeginn) |
/// | Abmeldung / Lieferende / Beendigung der Zuordnung | `93` | Ende zum (Datum Vertragsende) |
/// | Kündigung | `93` | Ende zum — the Kündigungstermin |
/// | Stammdatenänderung | `157` | Änderung zum, Gültigkeit Beginndatum |
/// | WiM Messstellenbetrieb | `76` | Datum zum geplanten Leistungsbeginn |
///
/// `163`/`164` appear nowhere in this table: the MIG uses them for *Beginn* and
/// *Ende Messperiode* inside SG8/SG9, not for a SG4 process date.
pub(super) fn utilmd_dtm_qualifier(pid: u32) -> &'static str {
    use edi_energy::utilmd_codes::dtm;
    match pid {
        // ── Zuordnungs-Meldungen (GPKE Teil 2 § 2.1.2 Nr. 2 / 10 / 13) ────
        //
        // Three adjacent PIDs, three different SG4 date columns. 55036 / 44036
        // has none at all ([`utilmd_carries_sg4_date`]); the Beendigung names a
        // Vertragsende and the Aufhebung the *originally confirmed*
        // Vertragsbeginn (Bedingung [507]) — the two are not interchangeable,
        // and the default below would have given both `92`.
        55_037 | 44_037 => dtm::ENDE_ZUM,
        55_038 | 44_038 => dtm::BEGINN_ZUM,
        // 55611 admits **both**: `DTM+93` under `STS+7++ZC8` and `DTM+92` under
        // `ZH1`. The PID alone cannot choose, so the workflow passes
        // `dtm_qualifier` explicitly and this is only the fallback for a caller
        // that does not — „Ende zum", the commoner of the two (Nr. 11 ends an
        // assignment; Nr. 13 cancels a future one).
        55_611 => dtm::ENDE_ZUM,
        // Lieferbeginn: Anmeldung and its Bestätigung/Ablehnung.
        55_001..=55_003 | 55_013..=55_015 | 55_077 | 55_078 | 55_080 => dtm::BEGINN_ZUM,
        44_001..=44_003 | 44_013..=44_015 => dtm::BEGINN_ZUM,
        // Lieferende von LF an NB, Lieferende von NB an LF, Beendigung der
        // Zuordnung, Kündigung — every one of them names a Vertragsende.
        55_004..=55_012 | 55_016..=55_018 => dtm::ENDE_ZUM,
        44_004..=44_012 | 44_016..=44_018 => dtm::ENDE_ZUM,
        // ── WiM Messstellenbetrieb ────────────────────────────────────────
        //
        // **Three different qualifiers, not one.** A Kündigung rendered with
        // `DTM+76` names a Leistungsbeginn where the AHB requires a
        // Vertragsende, and the receiver rejects it.
        //
        // | Anwendungsfall | Qualifier | AHB |
        // |---|---|---|
        // | Kündigung Messstellenbetrieb | `93` Datum Vertragsende ¹ | Strom 2.2 Kap. 10.1 / Gas 1.2 Kap. 6.1 |
        // | Anmeldung Messstellenbetrieb | `76` Lieferdatum/-zeit, geplant | Strom 2.2 Kap. 10.2 / Gas 1.2 Kap. 6.2 |
        // | Ende Messstellenbetrieb | `93` Datum Vertragsende ² | Strom 2.2 Kap. 10.4 / Gas 1.2 Kap. 6.4 |
        // | Verpflichtungsanfrage | `76` Lieferdatum/-zeit, geplant | Strom 2.2 Kap. 10.3 / Gas 1.2 Kap. 6.5 |
        //
        // ¹ XOR `DTM+471` „Ende zum nächstmöglichem Termin", which the workflow
        //   sets explicitly when the Kündigung names no fixed date; the
        //   Ablehnung additionally carries `157`/`Z01`/`Z10` under `Z12`.
        // ² alongside `DTM+92` Datum Vertragsbeginn, which the Abmeldung also
        //   carries.
        55_039..=55_041 | 44_039..=44_041 => dtm::ENDE_ZUM,
        55_051..=55_053 | 44_051..=44_053 | 44_183 => dtm::ENDE_ZUM,
        55_042..=55_044 | 44_042..=44_044 => dtm::LEISTUNGSBEGINN_GEPLANT,
        55_168..=55_170 | 44_168 | 44_169 => dtm::LEISTUNGSBEGINN_GEPLANT,
        // Two families inside the 556xx block are GPKE **Teil 2**, not Teil 4,
        // and both mark `SG4 DTM+92` „Datum Vertragsbeginn" Muss: Neuanlage
        // (55600–55605) and Ankündigung Zuordnung LF (55607/55608). Their AHB
        // does not define `157` at all. 55609 carries no SG4 date; the one
        // emitted here is an unlisted segment, not a missing Muss.
        55_600..=55_609 => dtm::BEGINN_ZUM,
        // ── NZR-EMob / Modell 2 (UTILMD AHB Strom 2.2 Kap. 11) ────────────
        //
        // The Anmeldung and its answer name a Vertragsbeginn; everything that
        // ends something — the Beendigung der Zuordnung, the Abmeldung and
        // both their answers — names a Vertragsende. The default below would
        // have given all six `92`, which is the wrong column on four of them.
        //
        // 55235/55237 (Zuordnung des ZP der NGZ zur NZR) begin an assignment,
        // 55236 ends one. They are MaBiS rather than Modell 2, but they share
        // this table.
        55_235 | 55_237 | 55_238 | 55_239 => dtm::BEGINN_ZUM,
        55_236 | 55_240..=55_243 => dtm::ENDE_ZUM,
        // Stammdatenänderung (GPKE Teil 4 / GeLi Gas): Änderung zum.
        55_109 | 55_110 | 55_136 | 55_137 | 55_610..=55_699 => dtm::AENDERUNG_ZUM,
        44_109..=44_199 => dtm::AENDERUNG_ZUM,
        // A PID with no entry here would otherwise get a silently wrong
        // qualifier; `Beginn zum` is the least surprising default and the
        // `utilmd_dtm_qualifier_covers_every_rendered_pid` test keeps the list
        // honest for everything mako actually sends.
        _ => dtm::BEGINN_ZUM,
    }
}
