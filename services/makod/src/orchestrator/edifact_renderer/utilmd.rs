//! UTILMD renderer.
//!
//! Split out of the flat `edifact_renderer` module; shared envelope and
//! payload-extraction helpers live in `super`.

use mako_engine::payload::{Tranchengroesse as TranchenPayload, UtilmdPayload, ZugeordneterMsb};

use super::*;
// ── UTILMD ────────────────────────────────────────────────────────────────────

/// Render a UTILMD outbound message from its typed payload,
/// [`UtilmdPayload`] — the struct the producing workflow built, so a field
/// renamed on one side is a compile error on the other.
///
/// Payload fields:
///
/// | Field           | Required | Description                                  |
/// |-----------------|----------|----------------------------------------------|
/// | `pid`           | yes      | Prüfidentifikator (u32)                       |
/// | `sender`        | yes      | Sender MP-ID (our own)                        |
/// | `receiver`      | no       | Receiver MP-ID (falls back to `msg.recipient`) |
/// | `malo` / `melo` | yes*     | Lokations-ID → `SG5 LOC+Z16` / `LOC+Z17`      |
/// | `vorgangsnummer`| no       | `IDE+24` DE 7402 (defaults to the message ref) |
/// | `referenz_vorgangsnummer` | on answers | `SG4 SG6 RFF+TN` — the **request's** `IDE+24` |
/// | `geplantes_produktpaket` | on a Bestätigung Anmeldung | `SG4 SG6 RFF+Z60` — the Produktpaket-ID the NB will implement |
/// | `process_date`  | yes      | Process date — the German calendar date (`YYYYMMDD` or `YYYY-MM-DD`); goes out as the UTC instant that day begins in the Sparte |
/// | `document_date` | no       | Document date (defaults to today at dispatch time) |
/// | `message_ref`   | no       | Derived from `causation_event_id` when absent  |
/// | `transaktionsgrund` | no   | `SG4 STS+7` DE 9013 element 2                  |
/// | `transaktionsgrund_ergaenzung` | **on 55077/55601** | `STS+7` DE 9013 element 3 (`ZW3`…`ZAP`); defaults to `ZW4` only where the column lists it, and is refused where it does not |
/// | `antwort_code`  | no       | `SG4 STS+E01` DE 9013 — **required on every Antwort-PID** |
/// | `antwort_codeliste` | no   | `STS+E01` DE 1131, the **Codeliste** the code comes from (`E_0622`, `S_0090`, `G_0051`, …) |
/// | `bemerkung`     | no       | `FTX+ACB` free text (mandatory alongside a catch-all Ablehnungscode) |
/// | `bilanzkreis`   | on 55001/55014/55608, 44001 | Strom: `SG8 SEQ+Z79` Produktpaket · Gas: `SG10 CCI+Z19` — its own slot, never `bemerkung` |
/// | `tranchengroesse` | **on `ZW2`** | `SG8 SEQ+Z79` Produkt `9991000002090` — a bare string is the prozentuale Aufteilung, `{art, wert}` names one of the other two |
/// | `document_code` | no       | `BGM` DE 1001, when the Anwendungsfall fixes one other than `E01` |
/// | `lokationstyp`  | no       | `SG5 LOC` DE 3227 — `Z21` Tranche, Gas `172` Meldepunkt; defaults to the PID's own object |
/// | `beteiligte_marktpartner` | on 55036/55038, 44036/44038 | `SG12 NAD+VY` — every Altlieferant, or the auslösender Marktpartner |
/// | `kunde_name` | on 55010 (`ZW4`/`ZAP`) | `SG12 NAD+Z09` „Kunde des LF" — a **name**, in `C080`, not an MP-ID |
/// | `kunde_namensformat` | with `kunde_name` | `NAD` DE 3045 — `Z01` Person, `Z02` Firma; defaults to `Z01` |
/// | `kunde_anschrift` | on 55001 | `SG12 NAD+Z04` Korrespondenzanschrift — `{strasse, plz, ort, land}` beside `kunde_name` |
/// | `kundengruppe` | on 55001, 55014 | `SG8 SEQ+Z01 CCI+++<code>` Gruppenzuordnung nach EnWG — `Z15` Haushaltskunde, `Z18` kein Haushaltskunde |
/// | `versorgungsart` | on 55014 | `SG8 SEQ+Z01 CCI+Z36++<code>` Versorgungsart — `ZC9` Ersatzversorgung, `ZD0` Grundversorgung, `ZE3` Ersatzbelieferung — in the block of the Gruppenzuordnung |
/// | `enfg_grundlage` | on 55001, 55014 | `SG8 SEQ+Z75 CCI+Z61++<code>` — `ZF9` erfüllt die Voraussetzung nach EnFG, `ZG0` erfüllt nicht, `ZG1` keine Angabe |
/// | `enfg_grund` | with `ZF9` | `CAV+<code>` the § of the privilege (`ZU5` § 21 Stromspeicher … `ZV3` § 39 Landstrom) |
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
    let u: UtilmdPayload = typed_payload(p)?;
    let pid = u.pid;

    // A **Listennachricht** is a different message shape: an `IDE+Z01` head
    // that is the Geschäftsvorfall, with one `IDE+24` member per disputed
    // Marktlokation under it. Rendered by its own path below.
    if !u.positionen.is_empty() || !u.listenmitglieder.is_empty() {
        return render_utilmd_liste(&u, msg);
    }

    let sender = u.sender.as_str();
    let receiver = u.receiver.as_deref().unwrap_or(msg.recipient.as_ref());

    // Determine UTILMD release track from PID: 44xxx = Gas, everything else = Strom.
    let track = if (44_000..=44_999).contains(&pid) {
        ReleaseTrack::Gas
    } else {
        ReleaseTrack::Strom
    };
    let release = release_for(msg, MessageType::Utilmd, track)?;

    // The MSB is assigned to the **Messlokation**, never to the Marktlokation
    // („Der MSB ist ausschließlich dem Objekt Messlokation zugeordnet" — WiM
    // Strom Teil 1 Kap. 2.1.2 d, AWH WiM Gas 2.0 Kap. 3.1.2 d), so every WiM
    // MSB-Wechsel message names a MeLo in `SG5 LOC` — the Anfrage and both
    // answers, in both Sparten. Everything else names a MaLo.
    let names_messlokation = mako_wim::geraetewechsel::wim_sparte(pid).is_some()
        || mako_wim::antwort_pid_meaning(pid)
            .is_some_and(|(request, _)| mako_wim::geraetewechsel::wim_sparte(request).is_some());
    let location_id_key = if names_messlokation { "melo" } else { "malo" };
    let named_location = if names_messlokation {
        u.melo.as_deref()
    } else {
        u.malo.as_deref()
    };
    // A MaBiS Vorgang is about a **MaBiS-Zählpunkt** and names no Marktlokation
    // at all — the ZP-Lifecycle Anfragen and their answers carry only
    // `mabis_zaehlpunkt`. Requiring a MaLo there refuses the whole message, and
    // a UTILMD that does not render is raw JSON on the AS4 leg.
    let location_id = match named_location {
        Some(id) if !id.is_empty() => Some(id),
        _ => match u.mabis_zaehlpunkt.as_deref() {
            Some(zp) if !zp.is_empty() => Some(zp),
            // Stammdaten carried whole name their own Lokationen.
            _ if utilmd_requires_sg5_loc(&release, pid) && u.stammdaten.lokationen.is_empty() => {
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
    let primary_is_mabis_zp = location_id.is_some() && named_location.is_none_or(str::is_empty);

    // `SG4 DTM` is not universal. UTILMD AHB Strom Kap. 8.11 / Gas Kap. 5.8
    // leave both the „Beginn zum" and „Ende zum" columns empty for the
    // Information über existierende Zuordnung (55036 / 44036): it names the LFA
    // and the Vorgang it refers to, and no date of its own. Emitting one there
    // is an unlisted segment, so the field is required everywhere else and
    // refused here.
    let dtm_qualifier = u
        .dtm_qualifier
        .as_deref()
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| utilmd_dtm_qualifier(pid));
    let carries_process_date = utilmd_carries_sg4_date(&release, pid, dtm_qualifier);
    let process_date = if carries_process_date {
        Some(
            u.process_date
                .as_deref()
                .ok_or_else(|| RenderError::MissingField {
                    message_type: mt.into(),
                    field: "process_date".into(),
                })?,
        )
    } else {
        None
    };

    let doc_date_owned = u
        .document_date
        .as_deref()
        .map(|d| dokumentdatum_303(mt, d))
        .transpose()?;
    let message_ref = u
        .message_ref
        .as_deref()
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let edifact_pid = Pruefidentifikator::new(pid).map_err(|e| RenderError::MissingField {
        message_type: mt.into(),
        field: format!("pid value {pid} is invalid: {e}").into(),
    })?;

    // The `SG4 DTM` DE 2005 qualifier is per-PID for almost everything, and the
    // table below is that mapping. A 55611 is the exception: „Beginn zum" under
    // `STS+7++ZH1` and „Ende zum" under `ZC8` (UTILMD AHB Strom Kap. 8.11
    // Bedingungen `[475]` / `[474]`), so the qualifier follows the **Grund** and
    // only the workflow knows it. An explicit value therefore wins.
    //
    // A `303` process date is the UTC instant at which the German day begins —
    // 00:00 Berlin in Strom, the Gastag's 06:00 in Gas (Allgemeine Festlegungen
    // V6.1d Kap. 3.1); `154` is a `102` date and goes out as one.
    let sparte = match track {
        ReleaseTrack::Gas => mako_fristen::Sparte::Gas,
        _ => mako_fristen::Sparte::Strom,
    };
    let process_date_wire = process_date
        .map(|d| {
            if edi_energy::utilmd_codes::dtm::sg4_format(dtm_qualifier) == "303" {
                prozessdatum_303(mt, "process_date", d, sparte)
            } else {
                Ok(normalise_date(d))
            }
        })
        .transpose()?;

    // `SG4 SG6 RFF+Z13` carries the Prüfidentifikator and the builder emits it
    // per Vorgang from `pruefidentifikator` — DE 1154 is `R n5`, so nothing but
    // the five-digit code belongs there.
    let mut builder = builders::UtilmdBuilder::new(release.clone())
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
    //
    // Where the workflow states none and the column admits exactly one, that
    // one is written: the Bestätigung Ende MSB 55052 is `E02` (UTILMD AHB Strom
    // 2.2 Kap. 10.4).
    let fixed_document_code = column_fixed_code(&release, pid, "BGM", None, "1001");
    let document_code = u.document_code.as_deref().or(match pid {
        55_062..=55_064 => Some(edi_energy::utilmd_codes::BGM_MABIS_ZP_LIFECYCLE),
        _ => fixed_document_code.as_deref(),
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
    let vorgangsnummer = u.vorgangsnummer.as_deref().unwrap_or(message_ref.as_str());

    let mut tx = builder.transaction(vorgangsnummer);
    // `SG4 IMD++Z36+Z12` „Identifikationslogik: Marktlokations-ID" — Muss on the
    // Gas Anmeldung NN 44001 and the Kündigung 44016 (UTILMD AHB Gas 1.2 Kap.
    // 5.6, S. 44). A Vorgang that names its Marktlokation by ID is identified by
    // it, and `SG5 LOC+172` carries that ID (Bedingung `[328]`); `Z13` „Alle
    // Identifikationsdaten" is the identification without one, which no mako
    // Vorgang sends.
    if track == ReleaseTrack::Gas
        && !names_messlokation
        && u.malo.as_deref().is_some_and(|m| !m.is_empty())
        && column_admits_code(
            &release,
            pid,
            "IMD",
            "7009",
            edi_energy::utilmd_codes::imd::MARKTLOKATIONS_ID,
        )
    {
        tx = tx.imd(
            edi_energy::utilmd_codes::imd::IDENTIFIKATIONSLOGIK,
            edi_energy::utilmd_codes::imd::MARKTLOKATIONS_ID,
        );
    }
    // `vertragsbeginn` — a `DTM+92` beside a `157` process date — precedes it:
    // the MIG places „Beginn zum" (Nr 00014) before „Änderung zum" (00017).
    // Like the process date it rides only where the column lists the place:
    // every GeLi Gas Änderung column makes both Muss, of the Antworten only
    // the 44176 lists them (UTILMD AHB Gas 1.2 Kap. 5.10–5.11), so a workflow
    // states the Änderung's dates and the column decides.
    if let Some(date) = u
        .vertragsbeginn
        .as_deref()
        .filter(|s| !s.is_empty())
        .filter(|_| {
            utilmd_carries_sg4_date(&release, pid, edi_energy::utilmd_codes::dtm::BEGINN_ZUM)
        })
    {
        tx = tx.date(
            edi_energy::utilmd_codes::dtm::BEGINN_ZUM,
            prozessdatum_303(mt, "vertragsbeginn", date, sparte)?,
        );
    }
    if let Some(date) = process_date_wire.as_deref() {
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
    // | `geraeteausbaudatum` | `206` | 55007/55008 with `STS+7++Z33` (AHB Strom 2.2 Kap. 8.10, S. 189, Bedingung `[79]`) |
    //
    // The third, `vertragsbeginn` (`92`), is written above, before the process
    // date: the GeLi Gas Antwort auf Anfrage 44142 restates both `DTM+92` and
    // `DTM+157` of the Anfrage — „Es ist das Datum/ Daten aus der Anfrage zu
    // verwenden" (AHB Gas 1.2 Kap. 5.12.1 Bedingung `[528]`).
    for (name, field, qualifier) in [
        (
            "bilanzierungsbeginn",
            u.bilanzierungsbeginn.as_deref(),
            edi_energy::utilmd_codes::dtm::BILANZIERUNGSBEGINN,
        ),
        (
            "bilanzierungsende",
            u.bilanzierungsende.as_deref(),
            edi_energy::utilmd_codes::dtm::BILANZIERUNGSENDE,
        ),
        (
            "geraeteausbaudatum",
            u.geraeteausbaudatum.as_deref(),
            edi_energy::utilmd_codes::dtm::GERAETEAUSBAUDATUM,
        ),
    ] {
        if let Some(date) = field.filter(|s| !s.is_empty()) {
            tx = tx.date(qualifier, prozessdatum_303(mt, name, date, sparte)?);
        }
    }

    // `SG4 SG6 RFF+TN` — „Referenz Vorgangsnummer (aus Anfragenachricht)",
    // Muss on every Antwortnachricht (UTILMD AHB Strom 2.2 / Gas 1.2). The
    // answer's own `IDE+24` must be a fresh number, so this is the only thing
    // that ties it to the request.
    if let Some(referenz) = u
        .referenz_vorgangsnummer
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        tx = tx.referenz_vorgangsnummer(referenz);
    }

    // `SG6 RFF+AAV` „Nummer der Anfrage" — Muss on the answers to a
    // Geschäftsdatenanfrage (UTILMD AHB Strom 2.2 Kap. 9.5): the ORDERS 17132's
    // `BGM` DE 1004, the only thing that ties the Stammdaten to the Anfrage.
    if let Some(anfrage) = u.anfrage_referenz.as_deref().filter(|s| !s.is_empty()) {
        tx = tx.reference(edi_energy::utilmd_codes::rff::NUMMER_DER_ANFRAGE, anfrage);
    }

    // `SG6 RFF+Z60` — Muss on a Bestätigung Anmeldung. Without it the LFN is
    // told its Anmeldung was accepted but not which of the Produktpakete it
    // offered the NB is going to implement.
    if let Some(paket) = u
        .geplantes_produktpaket
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        tx = tx.geplantes_produktpaket(paket);
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
    //
    // Where the payload states none and the column makes `STS+7` Muss with
    // exactly one code, that one is written — the GeLi Gas Antwort auf
    // Änderung restates its column's Grund (`ZE7` on a 44115, `ZI9` on a
    // 44176; UTILMD AHB Gas 1.2 Kap. 5.10–5.11): the column states it, nobody
    // guesses it.
    let fixed_grund = column_fixed_transaktionsgrund(&release, pid);
    if let Some(grund) = u.transaktionsgrund.as_deref().or(fixed_grund.as_deref()) {
        let t = if names_messlokation
            || track == ReleaseTrack::Gas
            || !column_lists_ergaenzung(&release, pid)
        {
            Transaktionsgrund::bare(grund)
        } else {
            let stated = u
                .transaktionsgrund_ergaenzung
                .as_deref()
                .filter(|s| !s.is_empty());
            let fixed_ergaenzung = match admitted_ergaenzungen(&release, pid).as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            };
            // `ZW4` „verbrauchende Marktlokation" is the default only where the
            // column admits it. 55077 and 55601 are the erzeugende pair and
            // admit `ZW0`/`ZW1`/`ZW2` — the Geschäftsvorfall — so a missing one
            // is a fact nobody stated rather than a value to guess: the three
            // mean different Zuordnungen and only the third builds a Tranche.
            let erg = match stated {
                Some(e) => e,
                None if ergaenzung_admitted(&release, pid, ergaenzung::VERBRAUCHENDE_MALO) => {
                    ergaenzung::VERBRAUCHENDE_MALO
                }
                // A column that admits one code states it: the Ablehnung of a
                // neue erzeugende Marktlokation 55605 is `ZW3` whatever the
                // Anmeldung's Geschäftsvorfall was (UTILMD AHB Strom 2.2).
                None if fixed_ergaenzung.is_some() => {
                    fixed_ergaenzung.as_deref().expect("checked by the guard")
                }
                None => {
                    let admitted = admitted_ergaenzungen(&release, pid);
                    return Err(RenderError::MissingField {
                        message_type: mt.into(),
                        field: format!(
                            "transaktionsgrund_ergaenzung on PID {pid}: the column does not \
                             admit the default {default} and lists {list} — the sender must \
                             state which one applies",
                            default = ergaenzung::VERBRAUCHENDE_MALO,
                            list = if admitted.is_empty() {
                                "no code".to_owned()
                            } else {
                                admitted.join("/")
                            },
                        )
                        .into(),
                    });
                }
            };
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
    //
    // Where the payload names no Codeliste and the PID's column fixes exactly
    // one — every Gas Antwort does, `G_0013` on a 44014 (UTILMD AHB Gas 1.2
    // Kap. 5.9) — that one is written: the column states it, nobody guesses it.
    if let Some(code) = u.antwort_code.as_deref() {
        let fixed = column_fixed_codeliste(&release, pid);
        let antwort = match u.antwort_codeliste.as_deref().or(fixed.as_deref()) {
            Some(cl) => AntwortStatus::from_codeliste(code, cl),
            None => AntwortStatus::bare(code),
        };
        tx = tx.antwort(antwort);

        // `SG4 STS+Z35` — „Status der Antwort des dritten Marktbeteiligten".
        //
        // **Muss when the Antwortcode is `A50` or `A57`** (UTILMD AHB Strom
        // Bedingungen `[356]` / `[84]`): both mean „der LFA hat der Anfrage zur
        // Beendigung der Zuordnung widersprochen", and GPKE Teil 2 Kap. 2.1.2 Nr. 6
        // requires the NB to state that refusal's ground alongside its own.
        // Refusing to render is the only way this stays true — an Ablehnung
        // that omits it tells the LFN its Anmeldung failed and not why the
        // incumbent would not release the Marktlokation, which is the one fact
        // it can act on.
        let dritter = u.dritter_antwortcode.as_deref();
        if edi_energy::utilmd_codes::CODES_REQUIRING_DRITTER.contains(&code) && dritter.is_none() {
            return Err(RenderError::MissingField {
                message_type: mt.into(),
                field: format!(
                    "antwort_code {code} requires \"dritter_antwortcode\" — UTILMD AHB Strom                      marks SG4 STS+Z35 Muss alongside it, and GPKE Teil 2 Kap. 2.1.2 Nr. 6 has the                      NB state the LFA's own Ablehnungsgrund"
                )
                .into(),
            });
        }
        if let Some(dritter_code) = dritter {
            let referenz = u
                .dritter_referenz_lokation
                .as_deref()
                .filter(|s| !s.is_empty());
            let objekt = u.dritter_objekt.as_deref().filter(|s| !s.is_empty());
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
    if let Some(text) = u.bemerkung.as_deref() {
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
    if let Some(bilanzkreis) = u.bilanzkreis.as_deref().filter(|s| !s.is_empty()) {
        tx = match track {
            ReleaseTrack::Gas => tx.merkmal(
                edi_energy::utilmd_codes::produkt::CCI_BILANZKREIS_GAS,
                bilanzkreis,
            ),
            // Geschäftsvorfall 3 („der LFN wird einer neu zu bildenden Tranche
            // zugeordnet") adds a second mandatory product to the same package.
            _ if is_tranchenbildung(&u) => {
                tx.produktpaket(edi_energy::utilmd_codes::Produktpaket::tranchenbildung(
                    bilanzkreis,
                    &tranchengroesse(&u, pid)?,
                ))
            }
            _ => {
                let mut paket = edi_energy::utilmd_codes::Produktpaket::bilanzkreis(bilanzkreis);
                paket.umsetzung = column_umsetzungsgrad(&release, pid);
                tx.produktpaket(paket)
            }
        };
    }

    // `SG12 NAD+Z09` „Kunde des LF" — a **name**, so it rides `C080` and not
    // the party-identification composite. Muss on a 55010 whose
    // Transaktionsgrundergänzung is `ZW4`/`ZAP` (Bedingung [279]); it is the
    // „Kundenname aus Anmeldung Lieferant neu" ([572]) the LFA compares against
    // its own contract holder at `E_0624` Prüfschritt 30.
    if let Some(name) = u.kunde_name.as_deref().filter(|s| !s.is_empty()) {
        let format = u
            .kunde_namensformat
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(edi_energy::utilmd_codes::namensformat::PERSON);
        // A `Z01` Personenname splits across the DE 3036 components the
        // column lists (Nachname, Vorname, …); a column marking one component
        // or a `Z02` Firmenbezeichnung takes the name in one line.
        let listed = |code: &str| {
            super::column_occurrences(
                MessageType::Utilmd,
                &release,
                pid,
                "NAD",
                Some(code),
                "3036",
            )
        };
        let parts = parts_of(
            name,
            format,
            listed(edi_energy::utilmd_codes::nad::KUNDE_DES_LF),
        );
        tx = tx.kunde_des_lf(parts, format);
        // `SG12 NAD+Z04` Korrespondenzanschrift des Kunden — Muss on a 55001
        // beside the name.
        if let Some(a) = u.kunde_anschrift.as_ref() {
            tx = tx.anschrift(
                edi_energy::utilmd_codes::nad::KORRESPONDENZANSCHRIFT_KUNDE,
                parts_of(
                    name,
                    format,
                    listed(edi_energy::utilmd_codes::nad::KORRESPONDENZANSCHRIFT_KUNDE),
                ),
                format,
                a.strasse.clone(),
                a.ort.clone(),
                a.plz.clone(),
                a.land.clone().unwrap_or_else(|| "DE".to_owned()),
            );
        }
    }

    // `SG8 SEQ+Z01` Daten der Marktlokation — the Gruppenzuordnung nach EnWG
    // (`CCI+++Z15` Haushaltskunde / `Z18` kein Haushaltskunde), Muss on a
    // 55001, and on a Bestätigung EoG 55014 the Versorgungsart `CCI+Z36`
    // after it. One block: the 55014 column lists `SEQ+Z01` „genau einmal je
    // SG4 IDE" (`[2061]`) with both `SG10` inside it (UTILMD AHB Strom 2.2
    // Kap. 8.6, S. 133, 136).
    let gruppe = u.kundengruppe.as_deref().filter(|s| !s.is_empty());
    let versorgungsart = u.versorgungsart.as_deref().filter(|s| !s.is_empty());
    if gruppe.is_some() || versorgungsart.is_some() {
        let mut block = tx.stammdaten(edi_energy::utilmd_codes::SEQ_DATEN_DER_MARKTLOKATION);
        if let Some(gruppe) = gruppe {
            block = block.cci("", gruppe);
        }
        if let Some(art) = versorgungsart {
            block = block.cci("Z36", art);
        }
        tx = block.done();
    }
    // `SG8 SEQ+Z75` Daten des Kunden des Lieferanten — `CCI+Z61` Grundlage
    // zur Verringerung der Umlagen nach EnFG (`ZF9` erfüllt, `ZG0` erfüllt
    // nicht, `ZG1` keine Angabe) and, where a privilege is claimed, `CAV` with
    // its § (`ZU5` … `ZV3`).
    if let Some(grundlage) = u.enfg_grundlage.as_deref().filter(|s| !s.is_empty()) {
        let mut block = tx
            .stammdaten(edi_energy::utilmd_codes::SEQ_DATEN_DES_KUNDEN)
            .cci("Z61", grundlage);
        if let Some(grund) = u.enfg_grund.as_deref().filter(|s| !s.is_empty()) {
            block = block.cav(grund);
        }
        tx = block.done();
    }

    // `SG12 NAD+VY` — the beteiligte Marktpartner a Vorgang names beside sender
    // and receiver: every Altlieferant on a 55036/44036 (Bedingung [518]), the
    // auslösender Marktpartner on a 55038/44038 ([579]/[571]).
    for party in &u.beteiligte_marktpartner {
        tx = tx.beteiligter_marktpartner(party);
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
    let tx = match (location_id, u.lokationstyp.as_deref()) {
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
    let tx = match u
        .mabis_zaehlpunkt
        .as_deref()
        .filter(|s| !s.is_empty() && !primary_is_mabis_zp)
    {
        Some(zp) => tx.location(edi_energy::Lokationstyp::MabisZaehlpunkt, zp),
        None => tx,
    };

    // Stammdaten carried whole: every `SG5 LOC`, `SG6` segment, `SG8` block and
    // `SG12` segment as the sender holds them — the answer to a
    // Geschäftsdatenanfrage, and the Anmeldung EoG 55013 / 44013.
    let mut tx = tx;
    for lokation in &u.stammdaten.lokationen {
        let typ = edi_energy::Lokationstyp::from_qualifier_code(&lokation.qualifier).ok_or_else(
            || RenderError::MissingField {
                message_type: mt.into(),
                field: format!(
                    "stammdaten.lokationen: {:?} is not a LOC DE 3227 qualifier",
                    lokation.qualifier
                )
                .into(),
            },
        )?;
        tx = tx.location(typ, &lokation.id);
    }
    for segment in &u.stammdaten.sg6 {
        tx = tx.sg6_segment(&segment.tag, segment.elemente.clone());
    }
    for block in &u.stammdaten.bloecke {
        let mut sg8 = tx.stammdaten(&block.seq);
        for segment in &block.segmente {
            sg8 = sg8.segment(&segment.tag, segment.elemente.clone());
        }
        tx = sg8.done();
    }
    for segment in &u.stammdaten.sg12 {
        tx = tx.sg12_segment(&segment.tag, segment.elemente.clone());
    }

    // `SG5 LOC+Z17` — every Messlokation the Marktlokation's Energiemenge is
    // computed from (Bedingung `[623]`), and with them the `SG8 SEQ+Z98`
    // „Daten der Marktlokation" and `SEQ+ZF3` „Daten der Messlokation" blocks.
    // Every Bestätigung Anmeldung makes all three Muss; behind `ZW7`
    // „Gemessene Marktlokation" the AHB says so explicitly (`[483]`), which is
    // why an answer that classifies the MaLo as gemessen and names no
    // Messlokation is refused here rather than at the counterparty.
    let melos = &u.messlokationen;
    let gemessen = u.transaktionsgrund_ergaenzung.as_deref()
        == Some(edi_energy::utilmd_codes::ergaenzung::GEMESSENE_MALO);
    // Stammdaten carried whole name their Messlokationen in their own `SG5
    // LOC+Z17` (the Anmeldung EoG 55013), so the check reads both.
    let carries_messlokation = u
        .stammdaten
        .lokationen
        .iter()
        .any(|l| l.qualifier == edi_energy::Lokationstyp::Messlokation.qualifier_code());
    if gemessen && melos.is_empty() && !names_messlokation && !carries_messlokation {
        return Err(RenderError::MissingField {
            message_type: mt.into(),
            field: "messlokationen — a ZW7 \u{201e}Gemessene Marktlokation\u{201c} answer must \
                    name the Messlokationen its Energiemenge is computed from, each with its \
                    Messstellenbetreiber (UTILMD AHB Strom Bedingungen [483] and [623])"
                .into(),
        });
    }
    for melo in melos.iter().filter(|m| !m.melo_id.is_empty()) {
        tx = tx.messlokation(&melo.melo_id);
    }
    if !melos.is_empty() {
        // `SG8 SEQ+Z98` — Daten der Marktlokation, carrying the MSB the NB has
        // assigned to it. „Zugeordneter Marktpartner" is Muss inside it.
        let msb = msb_of(u.malo_msb.as_ref()).ok_or_else(|| RenderError::MissingField {
            message_type: mt.into(),
            field: "malo_msb \u{2014} the SG8 SEQ+Z98 block a ZW7 answer opens makes \
                        SG10 CCI+++ZB3 \u{201e}Zugeordneter Marktpartner\u{201c} Muss"
                .into(),
        })?;
        tx = tx
            .stammdaten(edi_energy::utilmd_codes::SEQ_DATEN_DER_MARKTLOKATION_ANTWORT)
            .zugeordneter_msb(msb.mp_id, msb.rolle, msb.grundlage)
            .done();
        // `SG8 SEQ+ZF3` — one Datenblock per Messlokation (Bedingung [2284]),
        // referencing it by `RFF+Z19` and naming its own MSB.
        for melo in melos.iter().filter(|m| !m.melo_id.is_empty()) {
            let id = melo.melo_id.as_str();
            let msb = msb_of(melo.msb.as_ref()).ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: format!(
                    "messlokationen[{id}].msb \u{2014} SG8 SEQ+ZF3 makes SG10 CCI+++ZB3 Muss"
                )
                .into(),
            })?;
            tx = tx
                .stammdaten(edi_energy::utilmd_codes::seq_antwort::DATEN_DER_MESSLOKATION)
                .rff(edi_energy::utilmd_codes::rff_lokation::MESSLOKATION, id)
                .zugeordneter_msb(msb.mp_id, msb.rolle, msb.grundlage)
                .grundzustaendiger_msb(msb.gmsb_mp_id)
                .done();
        }
    }

    finish_interchange(tx.done().serialize(), sender, receiver, msg)
}

/// `{ "mp_id", "rolle", "grundlage", "gmsb_mp_id" }` as an answer payload
/// carries a `ZugeordneterMsb` — the four codes `SG10 CCI+++ZB3` needs.
struct Msb<'a> {
    mp_id: &'a str,
    rolle: &'a str,
    grundlage: &'a str,
    gmsb_mp_id: &'a str,
}

fn msb_of(v: Option<&ZugeordneterMsb>) -> Option<Msb<'_>> {
    let v = v?;
    fn nonempty(s: &Option<String>) -> Option<&str> {
        s.as_deref().filter(|s| !s.is_empty())
    }
    let mp_id = Some(v.mp_id.as_str()).filter(|s| !s.is_empty())?;
    Some(Msb {
        mp_id,
        rolle: nonempty(&v.rolle).unwrap_or(edi_energy::utilmd_codes::msb::GRUNDZUSTAENDIG),
        grundlage: nonempty(&v.grundlage)
            .unwrap_or(edi_energy::utilmd_codes::msb::GRUNDLAGE_VERTRAG),
        gmsb_mp_id: nonempty(&v.gmsb_mp_id).unwrap_or(mp_id),
    })
}

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
    u: &UtilmdPayload,
    msg: &OutboxMessage,
) -> Result<RenderedInterchange, RenderError> {
    let mt = "UTILMD";
    let pid = u.pid;
    fn required<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str, RenderError> {
        value.as_deref().ok_or_else(|| RenderError::MissingField {
            message_type: "UTILMD".into(),
            field: field.into(),
        })
    }

    let sender = u.sender.as_str();
    let receiver = u.receiver.as_deref().unwrap_or(msg.recipient.as_ref());
    let zaehlpunkt = required(&u.mabis_zaehlpunkt, "mabis_zaehlpunkt")?;
    let version = required(&u.zeitreihen_version, "zeitreihen_version")?;
    let listennummer = required(&u.listennummer, "listennummer")?;

    let release = release_for(msg, MessageType::Utilmd, ReleaseTrack::Strom)?;
    let edifact_pid = Pruefidentifikator::new(pid).map_err(|e| RenderError::MissingField {
        message_type: mt.into(),
        field: format!("pid value {pid} is invalid: {e}").into(),
    })?;
    let message_ref = u
        .message_ref
        .as_deref()
        .map(msg_ref_from_uuid)
        .unwrap_or_else(|| msg_ref_from_uuid(&msg.causation_event_id.to_string()));

    let mut builder = builders::UtilmdBuilder::new(release.clone())
        .sender(sender)
        .receiver(receiver)
        .pruefidentifikator(edifact_pid)
        .message_ref(message_ref)
        // `BGM+Z05` — every Clearingliste and every answer to one.
        .document_code(edi_energy::utilmd_codes::BGM_CLEARINGLISTE);
    if let Some(dd) = u.document_date.as_deref() {
        builder = builder.document_date(dokumentdatum_303("UTILMD", dd)?);
    }
    // `DTM+157` in `610` — the Bilanzierungsmonat, `CCYYMM`.
    if let Some(monat) = u.billing_period.as_deref().filter(|s| !s.is_empty()) {
        builder = builder.gueltigkeit_beginn(monat);
    }
    // `SG1 RFF+AAV` „Nummer der Anfrage" — Muss on a list the Netzbetreiber
    // distributes: the `BGM` DE 1004 of the ORDERS that ordered it (UTILMD AHB
    // Strom 2.2 Kap. 13.2, Bedingung `[503]`, S. 1110).
    if let Some(anfrage) = u.anfrage_referenz.as_deref().filter(|s| !s.is_empty()) {
        builder = builder.rff(edi_energy::utilmd_codes::rff::NUMMER_DER_ANFRAGE, anfrage);
    }

    // ── The head ─────────────────────────────────────────────────────────────
    let mut head = builder
        .list_transaction(listennummer)
        .location(edi_energy::Lokationstyp::MabisZaehlpunkt, zaehlpunkt)
        .summenzeitreihe_version(version);
    // `SG6 RFF+TN` — „Es ist die Listennummer aus der Lieferanten- bzw.
    // Bilanzierungsgebietsclearingliste zu verwenden" (Bedingung `[631]`).
    if let Some(referenz) = u.referenz_listennummer.as_deref().filter(|s| !s.is_empty()) {
        head = head.referenz_vorgangsnummer(referenz);
    }
    let mut builder = head.done();

    // ── The members ──────────────────────────────────────────────────────────
    let codeliste = if u.positionen.is_empty() {
        ""
    } else {
        required(&u.antwort_codeliste, "antwort_codeliste")?
    };
    for (i, pos) in u.positionen.iter().enumerate() {
        let malo = Some(pos.malo.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: format!("positionen[{i}].malo").into(),
            })?;
        let code = Some(pos.antwort_code.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: format!("positionen[{i}].antwort_code").into(),
            })?;
        // `IDE+24` DE 7402 must be unique across every Vorgangsnummer ever
        // sent, so it is minted from the list number and the position rather
        // than reusing the Marktlokations-ID.
        let vorgangsnummer = pos
            .vorgangsnummer
            .as_deref()
            .map_or_else(|| format!("{listennummer}-{}", i + 1), ToOwned::to_owned);
        let mut member = builder
            .transaction(vorgangsnummer)
            .antwort(edi_energy::utilmd_codes::AntwortStatus::from_codeliste(
                code, codeliste,
            ))
            .marktlokation(malo);
        if let Some(referenz) = pos
            .referenz_vorgangsnummer
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            member = member.referenz_vorgangsnummer(referenz);
        }
        builder = member.done();
    }

    // A list this participant distributes: one `IDE+24` per Marktlokation (or
    // Tranche) with its Bilanzierungszeitraum and its `SG8` blocks as the
    // sender holds them (Kap. 13.2, S. 1114–1121).
    for (i, mitglied) in u.listenmitglieder.iter().enumerate() {
        let malo = Some(mitglied.malo.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RenderError::MissingField {
                message_type: mt.into(),
                field: format!("listenmitglieder[{i}].malo").into(),
            })?;
        let vorgangsnummer = mitglied
            .vorgangsnummer
            .as_deref()
            .map_or_else(|| format!("{listennummer}-{}", i + 1), ToOwned::to_owned);
        let mut member = builder.transaction(vorgangsnummer).date(
            edi_energy::utilmd_codes::dtm::BILANZIERUNGSBEGINN,
            &mitglied.bilanzierungsbeginn,
        );
        if let Some(ende) = mitglied
            .bilanzierungsende
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            member = member.date(edi_energy::utilmd_codes::dtm::BILANZIERUNGSENDE, ende);
        }
        member = member.marktlokation(malo);
        if let Some(tranche) = mitglied.tranche.as_deref().filter(|s| !s.is_empty()) {
            member = member.location(edi_energy::Lokationstyp::Tranche, tranche);
        }
        for block in &mitglied.stammdaten.bloecke {
            let mut sg8 = member.stammdaten(&block.seq);
            for segment in &block.segmente {
                sg8 = sg8.segment(&segment.tag, segment.elemente.clone());
            }
            member = sg8.done();
        }
        builder = member.done();
    }

    finish_interchange(builder.serialize(), sender, receiver, msg)
}

/// Whether the Prüfschablone of `pid` lists an `SG5 LOC` at all.
///
/// Almost every UTILMD Vorgang is about a Lokation, and the exceptions are not
/// a list anyone should keep by hand: the **MaBiS Korrekturlisten** (55066,
/// 55196, 55202, 55224) answer a whole Clearingliste rather than a
/// Marktlokation, and every **Ablehnung** (55003, 55006, 55080) states that the
/// Anfrage failed and nothing about the Lokation it named. The AHB says which
/// is which, so this asks it instead of restating it.
///
/// Requiring a Lokation where the column lists none refuses the whole message,
/// and a UTILMD that does not render leaves raw JSON on the AS4 leg. Emitting
/// one anyway is `AHB-…-00050-LOC-NOT-PERMITTED` at the counterparty.
fn utilmd_requires_sg5_loc(release: &edi_energy::Release, pid: u32) -> bool {
    let Some((profile, af)) = utilmd_column(release, pid) else {
        return true;
    };
    // A column whose every `SG5` is conditional — 55605 names its
    // Marktlokation only beside `A12`/`A13` (Bedingung `[43]`), 55604 „wenn
    // bekannt" — admits an answer that names none; one with an unconditional
    // `SG5` (`Muss`, or `Muss [2061]` „genau einmal je SG4 IDE") does not.
    let unconditional = |s: &String| s == "Muss" || s == "Muss [2061]";
    profile
        .structure
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            edi_energy::profile::structure::Kind::Segment { nr, tag, .. } if tag == "LOC" => {
                Some(nr)
            }
            _ => None,
        })
        .any(|nr| {
            af.group_status("SG5", nr)
                .is_some_and(|st| st.iter().any(unconditional))
        })
}

/// Whether the Prüfschablone of `pid` lists the `SG4 DTM` place `qualifier`
/// would ride.
///
/// Not „does the column have any `SG4 DTM`": 55003 lists Nr 00033
/// „Lieferbeginndatum in Bearbeitung" and Nr 00034 „Datum für nächste
/// Bearbeitung" while refusing Nr 00023 „Beginn zum", so the question is about
/// the one place, not the group. The Information über existierende Zuordnung
/// (55036 / 44036) lists none at all: it names the LFA and the Vorgang it
/// refers to and no date of its own.
///
/// Emitting a date the column does not list is `AHB-…-DTM-NOT-PERMITTED`;
/// demanding one it does not list refuses the whole message, and a UTILMD that
/// does not render leaves raw JSON on the AS4 leg.
fn utilmd_carries_sg4_date(release: &edi_energy::Release, pid: u32, qualifier: &str) -> bool {
    let Some((profile, af)) = utilmd_column(release, pid) else {
        return true;
    };
    profile
        .structure
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| match &node.kind {
            edi_energy::profile::structure::Kind::Segment {
                nr,
                tag,
                discriminators,
                ..
            } if tag == "DTM"
                && discriminators
                    .iter()
                    .any(|d| d.codes.iter().any(|c| c == qualifier)) =>
            {
                Some((id, nr))
            }
            _ => None,
        })
        .filter(|(id, _)| profile.structure.path(*id).contains(&"SG4"))
        .any(|(_, nr)| af.segment_status(nr).is_some())
}

/// The `SG4 STS+7` Transaktionsgrund (DE 9013) the column of `pid` fixes:
/// the segment is unconditionally Muss there and admits exactly one code.
fn column_fixed_transaktionsgrund(release: &edi_energy::Release, pid: u32) -> Option<String> {
    let (profile, af) = utilmd_column(release, pid)?;
    let unconditional = |s: &String| s == "Muss" || s == "Muss [2061]";
    let mut codes: Vec<String> = profile
        .structure
        .layouts
        .iter()
        .filter(|l| {
            l.tag == "STS"
                && l.leaves()
                    .next()
                    .is_some_and(|(_, _, e)| e.codes.iter().any(|c| c.code == "7"))
                && af
                    .segment_status(&l.nr)
                    .is_some_and(|st| st.iter().any(unconditional))
        })
        .flat_map(|l| af.element_rules(&l.nr))
        .filter(|r| r.de == "9013")
        .flat_map(|r| r.operands.iter().filter_map(|o| o.code.clone()))
        .collect();
    codes.sort();
    codes.dedup();
    match codes.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The `STS+E01` DE 1131 Codeliste the column of `pid` fixes, when it lists
/// exactly one.
fn column_fixed_codeliste(release: &edi_energy::Release, pid: u32) -> Option<String> {
    column_fixed_code(release, pid, "STS", Some("E01"), "1131")
}

/// `true` when the column of `pid` lists `code` for data element `de` of
/// segment `tag`.
fn column_admits_code(
    release: &edi_energy::Release,
    pid: u32,
    tag: &str,
    de: &str,
    code: &str,
) -> bool {
    let Some((profile, af)) = utilmd_column(release, pid) else {
        return false;
    };
    profile
        .structure
        .layouts
        .iter()
        .filter(|l| l.tag == tag)
        .flat_map(|l| af.element_rules(&l.nr))
        .filter(|r| r.de == de)
        .any(|r| r.operands.iter().any(|o| o.code.as_deref() == Some(code)))
}

/// The one code the column of `pid` admits for data element `de` of segment
/// `tag` — the segment picked by the code its first data element carries
/// (`E01` for `STS+E01`), any when `qualifier` is `None`. `None` when the
/// column admits none or several.
fn column_fixed_code(
    release: &edi_energy::Release,
    pid: u32,
    tag: &str,
    qualifier: Option<&str>,
    de: &str,
) -> Option<String> {
    let (profile, af) = utilmd_column(release, pid)?;
    let mut codes: Vec<String> = profile
        .structure
        .layouts
        .iter()
        .filter(|l| {
            l.tag == tag
                && qualifier.is_none_or(|q| {
                    l.leaves()
                        .next()
                        .is_some_and(|(_, _, e)| e.codes.iter().any(|c| c.code == q))
                })
        })
        .flat_map(|l| af.element_rules(&l.nr))
        .filter(|r| r.de == de)
        .flat_map(|r| r.operands.iter().filter_map(|o| o.code.clone()))
        .collect();
    codes.sort();
    codes.dedup();
    match codes.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The `SG10 CCI+Z65` Umsetzungsgradvorgabe a one-package answer states.
///
/// `Z01` „vollumfänglich umzusetzen" unless the column demands exactly one
/// `Z02` in Paket `1P`: the Bestätigung of a Zuordnung to an erzeugende
/// Marktlokation 55608 and the Bestätigung EoG 55014 list DE 4051 as
/// `Z01 X [1P0..4]` / `Z02 X [1P1..1]` (UTILMD AHB Strom 2.2), so a single
/// Produktpaket there is the `Z02` one.
fn column_umsetzungsgrad(
    release: &edi_energy::Release,
    pid: u32,
) -> edi_energy::utilmd_codes::Umsetzungsgrad {
    use edi_energy::utilmd_codes::{Umsetzungsgrad, produkt};
    let demands_in_teilen = utilmd_column(release, pid).is_some_and(|(profile, af)| {
        profile
            .structure
            .layouts
            .iter()
            .filter(|l| l.tag == "CCI")
            .flat_map(|l| af.element_rules(&l.nr))
            .filter(|r| r.de == "4051")
            .flat_map(|r| r.operands.iter())
            .any(|o| {
                o.code.as_deref() == Some(produkt::UMSETZUNG_IN_TEILEN)
                    && o.operand.contains("[1P1..1]")
            })
    });
    if demands_in_teilen {
        Umsetzungsgrad::InTeilen
    } else {
        Umsetzungsgrad::Vollumfaenglich
    }
}

/// The UTILMD profile of `release` and the column of `pid` inside it.
fn utilmd_column(
    release: &edi_energy::Release,
    pid: u32,
) -> Option<(
    &'static edi_energy::Profile,
    &'static edi_energy::profile::model::Anwendungsfall,
)> {
    let code = edi_energy::Pruefidentifikator::new(pid).ok()?;
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(MessageType::Utilmd)
        .find(|p| p.release() == release)?;
    let af = profile.anwendungsfall(code.as_u32())?;
    Some((profile, af))
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
        // ── Zuordnungs-Meldungen (GPKE Teil 2 Kap. 2.1.2 Nr. 2 / 10 / 13) ────
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

/// A `Z01` Personenname splits across the five DE 3036 components as Nachname,
/// Vorname, …; a `Z02` Firmenbezeichnung is one line. The separator is the
/// caller's, so an unsplit name still renders.
fn parts_of(name: &str, format: &str, components: u8) -> Vec<String> {
    if format == edi_energy::utilmd_codes::namensformat::PERSON && components >= 2 {
        name.splitn(usize::from(components.min(5)), ',')
            .map(|s| s.trim().to_owned())
            .collect()
    } else {
        vec![name.to_owned()]
    }
}

/// Whether the column of `pid` lists the Transaktionsgrundergänzung — `STS+7`
/// DE 9013, second occurrence. Strom's Anmeldungen do (`ZW4` verbrauchende
/// Marktlokation); a Zuordnungs- or Stammdatenmeldung does not, and a code in
/// an element the column does not list is refused.
fn column_lists_ergaenzung(release: &edi_energy::Release, pid: u32) -> bool {
    let Ok(code) = edi_energy::Pruefidentifikator::new(pid) else {
        return true;
    };
    let Some(profile) = edi_energy::ReleaseRegistry::global()
        .profiles_for(MessageType::Utilmd)
        .find(|p| p.release() == release)
    else {
        return true;
    };
    let Some(af) = profile.anwendungsfall(code.as_u32()) else {
        return true;
    };
    profile
        .structure
        .layouts
        .iter()
        .filter(|l| {
            l.tag == "STS"
                && l.leaves()
                    .next()
                    .is_some_and(|(_, _, e)| e.codes.iter().any(|c| c.code == "7"))
        })
        .any(|l| {
            af.element_rules(&l.nr)
                .any(|r| r.de == "9013" && r.occurrence == 1)
        })
}

/// `true` when the payload states Geschäftsvorfall 3 (`STS+7++xxx+ZW2`).
///
/// The Tranchengröße hangs off the Transaktionsgrundergänzung and not off the
/// Prüfidentifikator: 55077 and 55601 each carry all three Geschäftsvorfälle,
/// and only the third one builds a Tranche.
fn is_tranchenbildung(u: &UtilmdPayload) -> bool {
    u.transaktionsgrund_ergaenzung.as_deref()
        == Some(edi_energy::utilmd_codes::ergaenzung::GESCHAEFTSVORFALL_3)
}

/// The `SG8` Tranchengröße a Geschäftsvorfall 3 must carry.
///
/// Codeliste der Konfigurationen 1.4 Kap. 6.1.1 makes the Produkt-Code
/// `9991000002090` „zwingend" on `STS+7++xxx+ZW2`, so an Anmeldung without one
/// is refused here rather than sent: the receiving NB has to reject it, and a
/// rejection costs the LF the Anmeldefrist.
///
/// The payload states which of the three Produkteigenschaften it is, because
/// they are not interchangeable — only the prozentuale Aufteilung is a share
/// `E_0623` can add up. `art` defaults to that form, being the one the
/// Aufteilungsfaktor and the Technische-Ressourcen lists are the exception to.
fn tranchengroesse(
    u: &UtilmdPayload,
    pid: u32,
) -> Result<edi_energy::utilmd_codes::Tranchengroesse, RenderError> {
    use edi_energy::utilmd_codes::Tranchengroesse;

    let missing = || RenderError::MissingField {
        message_type: "UTILMD".into(),
        field: "tranchengroesse".into(),
    };
    // A bare string is the prozentuale Aufteilung, which is what an LF sending
    // a percentage would write; the object form names the Eigenschaft.
    let (art, wert) = match u.tranchengroesse.as_ref().ok_or_else(missing)? {
        TranchenPayload::Prozent(s) => ("prozent", s.as_str()),
        TranchenPayload::Eigenschaft { art, wert } => {
            (art.as_deref().unwrap_or("prozent"), wert.as_str())
        }
    };
    if wert.is_empty() {
        return Err(missing());
    }
    match art {
        "prozent" => {
            // `[914] ∧ [930] ∧ [955]` — > 0, at most two decimals, < 100. A
            // share outside that is not a Tranche the NB can apply, and the
            // Prüfschablone cannot see it: the bounds live in the Codeliste.
            if !edi_energy::utilmd_codes::is_valid_prozent_wert(wert) {
                return Err(RenderError::MissingField {
                    message_type: "UTILMD".into(),
                    field: format!(
                        "tranchengroesse.wert {wert:?} on PID {pid}: the prozentuale \
                         Aufteilung must be > 0, < 100 and carry at most two decimals \
                         (Codeliste der Konfigurationen 1.4 Kap. 6.1.1)"
                    )
                    .into(),
                });
            }
            Ok(Tranchengroesse::prozent(wert))
        }
        "aufteilungsfaktor" => Ok(Tranchengroesse::aufteilungsfaktor(wert)),
        "technische_ressourcen" => Ok(Tranchengroesse::technische_ressourcen(wert)),
        other => Err(RenderError::MissingField {
            message_type: "UTILMD".into(),
            field: format!(
                "tranchengroesse.art {other:?}: the Codeliste publishes \
                 \"prozent\", \"aufteilungsfaktor\" and \"technische_ressourcen\""
            )
            .into(),
        }),
    }
}

/// The DE 9013 Transaktionsgrundergänzung codes a PID's column admits.
///
/// The Ergänzung is the occurrence **after** the Transaktionsgrund in `STS+7`,
/// which the profile indexes from zero — `occurrence == 1`. Reading occurrence
/// zero would answer with `E01`/`E03` and admit a code the column never lists
/// for the Ergänzung.
fn admitted_ergaenzungen(release: &edi_energy::Release, pid: u32) -> Vec<String> {
    let Some((profile, af)) = utilmd_column(release, pid) else {
        return Vec::new();
    };
    profile
        .structure
        .layouts
        .iter()
        .filter(|l| {
            l.tag == "STS"
                && l.leaves()
                    .next()
                    .is_some_and(|(_, _, e)| e.codes.iter().any(|c| c.code == "7"))
        })
        .flat_map(|l| af.element_rules(&l.nr))
        .filter(|r| r.de == "9013" && r.occurrence == 1)
        .flat_map(|r| r.operands.iter().filter_map(|o| o.code.clone()))
        .collect()
}

/// `true` when `code` is one the PID's column lists for the Ergänzung.
fn ergaenzung_admitted(release: &edi_energy::Release, pid: u32, code: &str) -> bool {
    admitted_ergaenzungen(release, pid)
        .iter()
        .any(|c| c == code)
}

/// What the pre-send AHB gate would say about a UTILMD a command is about to
/// write for a process running under `format_version`: render `pending` as
/// the outbox worker will, parse it back and validate it against its profile.
///
/// `Ok(findings)` — empty when the message passes. `Err` when it does not
/// render or parse at all. A command door that runs this before the spawn
/// refuses a request the gate would dead-letter, naming what is missing,
/// instead of writing a message nobody can send.
///
/// # Errors
///
/// The render or parse failure, as text.
pub(crate) fn utilmd_ahb_findings(
    pending: &mako_engine::outbox::PendingOutbox,
    tenant_id: mako_engine::ids::TenantId,
    format_version: &mako_engine::version::FormatVersion,
) -> Result<Vec<String>, String> {
    Ok(utilmd_gate_check(pending, tenant_id, format_version)?
        .findings
        .into_iter()
        .map(|e| e.message)
        .collect())
}

/// The UTILMD a command is about to write, rendered, and what its profile
/// says about it — the one check [`utilmd_ahb_findings`] answers in words and
/// a rehearsal (`POST /api/v1/probe/{command}`) answers per segment.
pub(crate) struct UtilmdGateCheck {
    /// The rendered interchange, as the gate parsed it.
    pub bytes: Vec<u8>,
    /// What keeps it from being sent — the profile's errors and every
    /// Formatbedingung finding; empty when the message passes.
    pub findings: Vec<edifact_rs::ValidationIssue>,
}

/// Render `pending` as the outbox worker will render it for a process running
/// under `format_version`, parse it back and validate it as the pre-send gate
/// does.
///
/// The worker renders a process step in the release of its process's format
/// version (`release_for`), so the check must too: rendered in the release in
/// force today, an answer to a process started before a cutover would be
/// judged against a column it is never sent in. A door spawning a new process
/// passes the format version it stamps on it — the one in force on its day.
///
/// # Errors
///
/// The render or parse failure, as text.
pub(crate) fn utilmd_gate_check(
    pending: &mako_engine::outbox::PendingOutbox,
    tenant_id: mako_engine::ids::TenantId,
    format_version: &mako_engine::version::FormatVersion,
) -> Result<UtilmdGateCheck, String> {
    use edi_energy::EdiEnergyMessage as _;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId};
    let process_id = ProcessId::new();
    let mut msg = OutboxMessage::new(
        StreamId::for_process(tenant_id, &process_id),
        process_id,
        tenant_id,
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type.clone(),
        pending.recipient.clone(),
        pending.payload.clone(),
    );
    msg.format_version = Some(format_version.clone());
    let rendered = render_utilmd(&pending.payload, &msg).map_err(|e| e.to_string())?;
    let parsed = edi_energy::parse(&rendered.bytes).map_err(|e| e.to_string())?;
    let report = parsed
        .validate_on_date(mako_fristen::heute())
        .map_err(|e| e.to_string())?;
    Ok(UtilmdGateCheck {
        findings: report.sending_refusals().cloned().collect(),
        bytes: rendered.bytes,
    })
}
