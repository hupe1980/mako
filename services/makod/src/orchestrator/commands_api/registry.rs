//! The command registry — single source of truth for name, Marktrollen, PID, dispatch fn.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;
// ── Command registry ──────────────────────────────────────────────────────────

// Registry of all supported ERP commands.
//
// Each entry binds a command name to its permitted Marktrollen, primary PID,
// and a typed dispatch function.  This single source of truth prevents the
// three parallel data structures that existed before F-022 from drifting apart:
// - a `(&str, &[&str])` role table
// - a `dispatch_command` match arm
// - a `command_primary_pid` match arm
//
// Adding a command here without supplying a `dispatch` function pointer is a
// **compile error**.  Stub commands that are not yet fully implemented carry an
// explicit `cmd_*` stub function that returns `NotImplemented`.
//
// Sources:
// - BDEW GPKE AHB (BK6-22-024, LFW24)
// - BDEW GeLi Gas AHB (BK7-24-01-009)
// - BDEW WiM AHB (BK6-22-024, Anlagen 2a/2b)
// - BDEW MABIS AHB (BK6-24-174)

pub(crate) static COMMAND_REGISTRY: &[CommandDescriptor] = &[
    // ── GPKE Lieferbeginn (electricity) ───────────────────────────────────────
    CommandDescriptor {
        name: "gpke.lieferbeginn.anmelden",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55001),
        dispatch: cmd_gpke_lieferbeginn_anmelden,
    },
    CommandDescriptor {
        name: "gpke.lieferbeginn.bestaetigen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55002),
        dispatch: cmd_gpke_lieferbeginn_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.lieferbeginn.ablehnen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55003),
        dispatch: cmd_gpke_lieferbeginn_ablehnen,
    },
    // GPKE Teil 2 § 2.2 Neuanlage. The answer window is 00:00 Uhr des 61. WT
    // nach dem ÜT — `E_0608` Prüfschritte 110/590 run a daily identification
    // Prüflauf for 60 Werktage before a refusal is admissible.
    CommandDescriptor {
        name: "gpke.neuanlage.bestaetigen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55602),
        dispatch: cmd_gpke_neuanlage_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.neuanlage.ablehnen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55604),
        dispatch: cmd_gpke_neuanlage_ablehnen,
    },
    // GPKE Teil 2 § 3.1 Bearbeitungsstand Abrechnungsdaten. One command: the
    // `E_0595` clusters say whether a Stammdatenänderung follows, not whether
    // the LF's Bestellung was granted, and IFTSTA 21047 carries both.
    CommandDescriptor {
        name: "gpke.abrechnungsdaten.bearbeitungsstand",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(21047),
        dispatch: cmd_gpke_abrechnungsdaten_bearbeitungsstand,
    },
    CommandDescriptor {
        name: "gpke.lieferbeginn.aktivieren",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: None,
        dispatch: cmd_gpke_lieferbeginn_aktivieren,
    },
    // ── GPKE Lieferende (electricity) ─────────────────────────────────────────
    CommandDescriptor {
        name: "gpke.lieferende.anmelden",
        permitted_roles: &[Marktrolle::Lf],
        // UTILMD 55004 „Abmeldung" (LF → NB), answered 55005/55006. It was
        // registered as 55002, which is the NB's *Bestätigung Anmeldung* — a
        // different message in the opposite direction, and the value Cedar
        // evaluates as the command's PID attribute.
        primary_pid: pid(55004),
        dispatch: cmd_gpke_lieferende_anmelden,
    },
    CommandDescriptor {
        name: "gpke.lieferende.bestaetigen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55005),
        dispatch: cmd_gpke_lieferende_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.lieferende.ablehnen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55006),
        dispatch: cmd_gpke_lieferende_ablehnen,
    },
    // ── GPKE Kündigung (LFN → LFA, EBD E_0614) ───────────────────────────────
    //
    // Both sides are the *supplier*: the incoming LF terminates the incumbent's
    // contract directly, and the incumbent answers 55017/55018 by the Ablauf
    // des 1. WT nach dem ÜT.
    CommandDescriptor {
        name: "gpke.kuendigung.anmelden",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55016),
        dispatch: cmd_gpke_kuendigung_anmelden,
    },
    CommandDescriptor {
        name: "gpke.kuendigung.bestaetigen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55017),
        dispatch: cmd_gpke_kuendigung_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.kuendigung.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55018),
        dispatch: cmd_gpke_kuendigung_ablehnen,
    },
    // ── GPKE NB-seitiges Lieferende (PID 55007 NB→LF) ────────────────────────
    // The NB sends PID 55007 (Ankündigung) via AS4; the LF responds.
    // APERAK Frist: 24h (BK6-22-024 §4).
    CommandDescriptor {
        name: "gpke.nb-lieferende.bestaetigen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55008),
        dispatch: cmd_gpke_nb_lieferende_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.nb-lieferende.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55009),
        dispatch: cmd_gpke_nb_lieferende_ablehnen,
    },
    // ── GPKE Beendigung der Zuordnung (PID 55010 NB→LFA, EBD E_0624) ─────────
    // The NB asks the LFA to end the network assignment; the LFA answers 55011
    // (Bestätigung) or 55012 (Ablehnung) within the 24 h Frist (BK6-22-024 §4).
    // The ingest dispatcher has always spawned this workflow on an inbound
    // 55010, but without these two commands the process had no answer path and
    // could only run out its deadline.
    CommandDescriptor {
        name: "gpke.beendigung-zuordnung.bestaetigen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55011),
        dispatch: cmd_gpke_beendigung_zuordnung_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.beendigung-zuordnung.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55012),
        dispatch: cmd_gpke_beendigung_zuordnung_ablehnen,
    },
    // ── GPKE Ersatz-/Grundversorgung (§36/§38 EnWG, PIDs 55013–55015) ─────────
    // The NB assigns a contractless MaLo to the Grundversorger; the E/G
    // answers with Bestätigung (stating Ersatz- vs. Grundversorgung + BK)
    // or Ablehnung (EBD E_0615: A02/A04/A05).
    CommandDescriptor {
        name: "gpke.eog.anmelden",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(55013),
        dispatch: cmd_gpke_eog_anmelden,
    },
    CommandDescriptor {
        name: "gpke.eog.bestaetigen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55014),
        dispatch: cmd_gpke_eog_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.eog.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55015),
        dispatch: cmd_gpke_eog_ablehnen,
    },
    // ── GPKE Ankündigung Zuordnung LF (PID 55607 NB→LFN) ─────────────────────
    // After Lieferantenwechsel the NB sends PID 55607 to the new LF (LFN).
    // LFN must respond within 24h (BK6-22-024 §4).
    CommandDescriptor {
        name: "gpke.zuordnung-lf.bestaetigen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55608),
        dispatch: cmd_gpke_zuordnung_lf_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.zuordnung-lf.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55609),
        dispatch: cmd_gpke_zuordnung_lf_ablehnen,
    },
    // ── GPKE MaLo-ID continuation (ERP callback after MaloIdentified event) ───
    // Primary key is `tx_id` from the `MaloIdentified` ERP event.  The engine
    // resolves malo_id + nb_mp_id from the mc_txres/ cache.
    CommandDescriptor {
        name: "maloid.lieferbeginn.fortsetzen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55001),
        dispatch: cmd_maloid_lieferbeginn_fortsetzen,
    },
    // ── GPKE Sperrung / Entsperrung (ORDERS 17115/17117) ─────────────────────
    //
    // Two sides, two workflows — do not conflate them:
    //
    //   LF side (`gpke-sperrung-lf`):  LF issues the order, awaits ORDRSP + IFTSTA.
    //     gpke.sperrung.beauftragen    → 17115 Sperrauftrag
    //     gpke.entsperrung.beauftragen → 17117 Entsperrauftrag
    //     gpke.sperrung.stornieren     → ORDCHG 39000
    //
    //   NB side (`gpke-sperrung`):     NB receives the order and reports execution.
    //     gpke.sperrung.bestaetigen    → executed      → IFTSTA 21039
    //     gpke.sperrung.fehlgeschlagen → not executed  → IFTSTA 21039 + reason
    //
    // `sperrd` calls the NB-side pair after field-service confirmation
    // (GPKE BK6-22-024 §5).
    CommandDescriptor {
        name: "gpke.sperrung.beauftragen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(17115),
        dispatch: cmd_gpke_sperrung_beauftragen,
    },
    CommandDescriptor {
        name: "gpke.entsperrung.beauftragen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(17117),
        dispatch: cmd_gpke_entsperrung_beauftragen,
    },
    CommandDescriptor {
        name: "gpke.sperrung.stornieren",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(39000),
        dispatch: cmd_gpke_sperrung_stornieren,
    },
    CommandDescriptor {
        name: "gpke.sperrung.bestaetigen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(17115),
        dispatch: cmd_gpke_sperrung_bestaetigen,
    },
    CommandDescriptor {
        name: "gpke.sperrung.fehlgeschlagen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(17115),
        dispatch: cmd_gpke_sperrung_fehlgeschlagen,
    },
    // ── GPKE Netznutzungsabrechnung — LF-payer side ───────────────────────────
    // The LF receives an INVOIC from the NB and settles or disputes it.
    // Routing key: `invoice_ref` (INVOIC message-reference) from the payload.
    CommandDescriptor {
        name: "gpke.abrechnung.annehmen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(31002),
        dispatch: cmd_gpke_abrechnung_annehmen,
    },
    CommandDescriptor {
        name: "gpke.abrechnung.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(31002),
        dispatch: cmd_gpke_abrechnung_ablehnen,
    },
    // Self-issued INVOIC 31006 (LF selbstausgestellt): `invoicd` generates the
    // BO4E Rechnung via `POST /api/v1/selbstausstellen/{malo_id}` and then calls
    // this command to record the outbound send and await the NB's REMADV response.
    // Payload: { "invoice_ref": "<uuid>", "nb_mp_id": "...", "sender_mp_id": "..." }
    CommandDescriptor {
        name: "gpke.abrechnung.selbstausstellen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(31006),
        dispatch: cmd_gpke_abrechnung_selbstausstellen,
    },
    // ── netzbilanzd outbound INVOIC generation (NB role) ──────────────────────
    // `netzbilanzd` generates NNE/MMM invoices and dispatches them to `makod`
    // via these commands.  `GpkeAbrechnungWorkflow` spawns in invoicer role so
    // the inbound REMADV from the LF can be routed back to the correct process.
    //
    // Payload: { "invoice_ref": "<rechnungsnummer>", "sender_mp_id": "<MP-ID>",
    //            "recipient_mp_id": "<MP-ID>", "pid": <PID>, "sparte": "STROM"|"GAS",
    //            "rechnung": <BO4E Rechnung JSON> }
    //
    // `invoice_ref` is the invoice number, not a UUID: it is the business key the
    // inbound REMADV correlates on, so it has to be the number printed on the
    // document the counterparty received.
    // PID 31001 — Abschlagsrechnung Netznutzung: a payment on account, settled
    // later by the Abschlussrechnung that deducts it (INVOIC AHB `SG50 MOA+113`
    // + `SG51 RFF+AFL`).
    CommandDescriptor {
        name: "invoic.nne-abschlag.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31001),
        dispatch: cmd_invoic_nne_abschlag_stellen,
    },
    // PID 31002 is the NN-Rechnung in **both** Sparten (INVOIC AHB), so it is
    // one command permitted to both roles — not two identical ones split by a
    // name. The Strom and Gas variants used to differ only in `permitted_roles`,
    // which meant a `GNB` deployment was refused the Strom-named command on role
    // grounds while dispatching the exact same PID through the same function.
    CommandDescriptor {
        name: "invoic.nne.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31002),
        dispatch: cmd_invoic_nne_stellen,
    },
    // PID 31005 is likewise Sparte-neutral: NB → LF Gas MMM shares it with
    // Strom. The aggregierte Gas MMM to the MGV is 31007/31008 (`gabi.mmm.*`).
    CommandDescriptor {
        name: "invoic.mmm.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31005),
        dispatch: cmd_invoic_mmm_stellen,
    },
    // PID 31009 is issued **by** the Messstellenbetreiber in all seven of its
    // Anwendungsfälle (PID overview 4.0), so `MSB` is a permitted role here.
    // Listing only `NB` made this a single-role command, which silently ignores
    // the asserted role — and, worse, locked out the one deployment shape that
    // most needs it: a `--marktrollen MSB` instance failed the licence check on
    // the invoice it is the only party entitled to send. `NB` stays because the
    // grundzuständige MSB is commonly the network operator's own arm.
    CommandDescriptor {
        name: "wim.msb-rechnung.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: pid(31009),
        dispatch: cmd_wim_msb_rechnung_stellen,
    },
    // ── GeLi Gas Lieferbeginn (gas) — LFG side (BK7-24-01-009) ───────────────
    CommandDescriptor {
        name: "geli.lieferbeginn.anmelden",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44001),
        dispatch: cmd_geli_lieferbeginn_anmelden,
    },
    CommandDescriptor {
        name: "geli.lieferbeginn.bestaetigen",
        permitted_roles: &[Marktrolle::Gnb],
        // UTILMD 44002 „Bestätigung Anmeldung NN". 44003 is the *Ablehnung*.
        primary_pid: pid(44002),
        dispatch: cmd_geli_lieferbeginn_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.lieferbeginn.ablehnen",
        permitted_roles: &[Marktrolle::Gnb],
        // UTILMD 44003 „Ablehnung Anmeldung NN". 44004 is the *Abmeldung*, an
        // inbound message from the supplier in the opposite direction.
        primary_pid: pid(44003),
        dispatch: cmd_geli_lieferbeginn_ablehnen,
    },
    // ── GeLi Gas Lieferende (gas) ─────────────────────────────────────────────
    CommandDescriptor {
        name: "geli.lieferende.anmelden",
        permitted_roles: &[Marktrolle::Lfg],
        // UTILMD 44004 „Abmeldung NN" (LF → GNB), answered 44005/44006.
        primary_pid: pid(44004),
        dispatch: cmd_geli_lieferende_anmelden,
    },
    // GeLi Gas 3.0 § 3.1 — the Neulieferant's Kündigung goes straight to the
    // Altlieferant, without the grid operator. The Strom twin is
    // `gpke.kuendigung.anmelden`.
    CommandDescriptor {
        name: "geli.kuendigung.anmelden",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44016),
        dispatch: cmd_geli_kuendigung_anmelden,
    },
    CommandDescriptor {
        name: "geli.lieferende.bestaetigen",
        permitted_roles: &[Marktrolle::Gnb],
        primary_pid: pid(44005),
        dispatch: cmd_geli_lieferende_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.lieferende.ablehnen",
        permitted_roles: &[Marktrolle::Gnb],
        primary_pid: pid(44006),
        dispatch: cmd_geli_lieferende_ablehnen,
    },
    // ── GeLi Gas — the answers the *supplier* owes ───────────────────────────
    //
    // The Gas answer commands above belong to the GNB; these are addressed
    // **to** the supplier. All are `Lfg`, like every other Gas command: the
    // Strom/Gas split is enforced at the role level.
    CommandDescriptor {
        // 44007 Abmeldung NN vom NB → 44008 / 44009 (Codeliste `E_3002`).
        name: "geli.nb-lieferende.bestaetigen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44008),
        dispatch: cmd_geli_abmeldung_nb_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.nb-lieferende.ablehnen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44009),
        dispatch: cmd_geli_abmeldung_nb_ablehnen,
    },
    CommandDescriptor {
        // 44010 Abmeldungsanfrage des NB → 44011 / 44012 (Codeliste `E_3020`).
        name: "geli.beendigung-zuordnung.bestaetigen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44011),
        dispatch: cmd_geli_abmeldungsanfrage_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.beendigung-zuordnung.ablehnen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44012),
        dispatch: cmd_geli_abmeldungsanfrage_ablehnen,
    },
    CommandDescriptor {
        // 44016 Kündigung beim alten Lieferanten → 44017 / 44018 (`E_3001`).
        // LFN → LFA, so both parties are suppliers and the GNB never sees it.
        name: "geli.kuendigung.bestaetigen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44017),
        dispatch: cmd_geli_kuendigung_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.kuendigung.ablehnen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44018),
        dispatch: cmd_geli_kuendigung_ablehnen,
    },
    CommandDescriptor {
        // 44013 Anmeldung EoG → 44014 / 44015 (`E_3008`) — how a Gas
        // Grundversorger answers an assignment under § 36 / § 38 EnWG.
        name: "geli.eog.bestaetigen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44014),
        dispatch: cmd_geli_eog_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.eog.ablehnen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44015),
        dispatch: cmd_geli_eog_ablehnen,
    },
    // ── GeLi Gas LF Stornierung (ERP-initiated, LF sends 44022 to GNB) ────────
    // LFN/LFA initiates a supply-change cancellation; ERP supplies `malo_id`
    // and optional `bgm_qualifier` (E01=Kündigung, E02=Rücktritt, E35=Sperrung).
    CommandDescriptor {
        name: "geli.stornierung.initiieren",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44022),
        dispatch: cmd_geli_gas_stornierung_initiieren,
    },
    // ── GeLi Gas Datenabruf (ORDERS 17103 — LF requests Brennwert/Zustandszahl) ─
    // LF sends ORDERS 17103 outbound to NB/MSB requesting Gas quality data.
    // Spawns a GeliGasDatanabrufWorkflow that tracks the 10-Werktage response.
    CommandDescriptor {
        name: "geli.datenabruf.anfragen",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(17103),
        dispatch: cmd_geli_gas_datenabruf_anfragen,
    },
    // ── GeLi Gas Ersatz-/Grundversorgung (GNB registers MaLo into E/G) ────────
    // GNB-initiator role: sends UTILMD G 44013 (EoG Anmeldung) to the E/G LF.
    // The Gas twin of `gpke.eog.anmelden`. Spawns GeliGasSupplierChangeWorkflow
    // and tracks the 10-Werktage response window (BK7-24-01-009).
    CommandDescriptor {
        name: "geli.eog.anmelden",
        permitted_roles: &[Marktrolle::Gnb],
        primary_pid: pid(44013),
        dispatch: cmd_geli_eog_anmelden,
    },
    // ── WiM Messstellenbetrieb ────────────────────────────────────────────────
    //
    // `wim.geraetewechsel.beauftragen` spawns an outbound MSB-Wechsel order; the
    // PID selects the direction (55039/55042 = MSB → NB, 55051/55168 = NB → MSB).
    // `.bestaetigen` / `.ablehnen` answer an *inbound* order via APERAK.
    CommandDescriptor {
        name: "wim.geraetewechsel.beauftragen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_geraetewechsel_beauftragen,
    },
    // ── Rechnungsabwicklung MSB über LF (WiM Strom Teil 1) ────────────────────
    // `.beenden` spawns an outbound ORDERS 17006; either side may end the
    // arrangement (AWH AD WiM V1.3 §§2.9/2.11), hence both roles. `.zustimmen`
    // / `.ablehnen` answer an inbound Beendigung with ORDRSP 19009/19010 —
    // the decision the counterparty's EBD (E_0206/E_0209) then checks.
    CommandDescriptor {
        name: "wim.rechnungsabwicklung.beenden",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Msb],
        primary_pid: pid(17006),
        dispatch: cmd_wim_rechnungsabwicklung_beenden,
    },
    CommandDescriptor {
        name: "wim.rechnungsabwicklung.zustimmen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Msb],
        primary_pid: pid(19009),
        dispatch: cmd_wim_rechnungsabwicklung_zustimmen,
    },
    CommandDescriptor {
        name: "wim.rechnungsabwicklung.ablehnen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Msb],
        primary_pid: pid(19010),
        dispatch: cmd_wim_rechnungsabwicklung_ablehnen,
    },
    // ── ESA Wertebestellung — ESA origination side ────────────────────────────
    // This deployment *is* the ESA. Consent-gated (esa_outbound) except the
    // Abbestellung, which is the GDPR Art. 7(3) revocation path.
    CommandDescriptor {
        name: "esa.werteanfrage.stellen",
        permitted_roles: &[Marktrolle::Esa],
        primary_pid: Some(mako_wim::esa_wertebestellung::ANFRAGE_PID),
        dispatch: cmd_esa_werteanfrage_stellen,
    },
    CommandDescriptor {
        name: "esa.bestellung.beauftragen",
        permitted_roles: &[Marktrolle::Esa],
        primary_pid: Some(mako_wim::esa_wertebestellung::BESTELLUNG_PID),
        dispatch: cmd_esa_bestellung_beauftragen,
    },
    CommandDescriptor {
        name: "esa.stornierung.beauftragen",
        permitted_roles: &[Marktrolle::Esa],
        primary_pid: Some(mako_wim::esa_wertebestellung::STORNIERUNG_PID),
        dispatch: cmd_esa_stornierung_beauftragen,
    },
    CommandDescriptor {
        name: "esa.abbestellung.beauftragen",
        permitted_roles: &[Marktrolle::Esa],
        primary_pid: Some(mako_wim::esa_wertebestellung::ABBESTELLUNG_PID),
        dispatch: cmd_esa_abbestellung_beauftragen,
    },
    // ── §20b EnWG Netzzugangsplattform (no PID — not an EDIFACT process) ──
    // Bestellung/Änderung/Abbestellung von Zählpunktanordnungen (Abs. 2 Nr. 1)
    // und Verrechnungskonzepten (Abs. 2 Nr. 2); Registrierung von
    // §42c-Vereinbarungen (Abs. 2 Nr. 3).
    CommandDescriptor {
        name: "netzzugang.zaehlpunktanordnung.beauftragen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Msb],
        primary_pid: None,
        dispatch: cmd_netzzugang_zaehlpunktanordnung,
    },
    CommandDescriptor {
        name: "netzzugang.verrechnungskonzept.beauftragen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Msb],
        primary_pid: None,
        dispatch: cmd_netzzugang_verrechnungskonzept,
    },
    CommandDescriptor {
        name: "netzzugang.energysharing.registrieren",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: None,
        dispatch: cmd_netzzugang_energysharing,
    },
    // MSB side: deliver Typ-2 values to the ESA (outbound MSCONS 13027, UC 4.2).
    // Gated on a confirmed Bestellung inside the workflow.
    CommandDescriptor {
        name: "wim.wertebestellung.liefern",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::WERTE_UEBERMITTLUNG_PID),
        dispatch: cmd_wim_wertebestellung_liefern,
    },
    // MSB side: answer the ESA's ordering handshake (QUOTES 15003 / ORDRSP
    // 19011-19014). These let mako drive the MSB half — a self-contained loopback.
    CommandDescriptor {
        name: "wim.wertebestellung.anbieten",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::ANGEBOT_PID),
        dispatch: cmd_wim_wertebestellung_anbieten,
    },
    CommandDescriptor {
        name: "wim.wertebestellung.anfrage-ablehnen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::ANGEBOT_PID),
        dispatch: cmd_wim_wertebestellung_anfrage_ablehnen,
    },
    CommandDescriptor {
        name: "wim.wertebestellung.bestellung-beantworten",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::BESTAETIGUNG_PID),
        dispatch: cmd_wim_wertebestellung_bestellung_beantworten,
    },
    CommandDescriptor {
        name: "wim.wertebestellung.stornierung-beantworten",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::STORNO_BESTAETIGUNG_PID),
        dispatch: cmd_wim_wertebestellung_stornierung_beantworten,
    },
    CommandDescriptor {
        name: "wim.wertebestellung.abbestellung-beantworten",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::BESTAETIGUNG_PID),
        dispatch: cmd_wim_wertebestellung_abbestellung_beantworten,
    },
    // The answering role depends on the inbound PID: 55042 (Anmeldung MSB,
    // MSBN → NB) is answered by the NB; 55039 (Kündigung MSB, MSBN → MSBA) by
    // the incumbent MSB. Multi-role — callers must supply `marktrolle`.
    CommandDescriptor {
        name: "wim.geraetewechsel.bestaetigen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_geraetewechsel_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.geraetewechsel.ablehnen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_geraetewechsel_ablehnen,
    },
    // The **technical** acknowledgement, on its own command because it is on
    // its own clock: 45 minutes for Strom UTILMD (APERAK AHB 1.0 §2.4.1),
    // against 3 / 5 / 7 / 1 Werktage for the business answer above.
    CommandDescriptor {
        name: "wim.geraetewechsel.aperak",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_geraetewechsel_aperak,
    },
    // ── WiM Mitteilung über Gesamtvorgang (IFTSTA 21009–21013) ───────────
    //
    // The Anmeldebestätigung 55043 is vorläufig; these two commands are what
    // makes a Zuordnung constitutive (WiM Teil 1 Kap. 2.1.1 / 2.3.2 Nr. 7/8).
    CommandDescriptor {
        name: "wim.gesamtvorgang.melden",
        permitted_roles: &[Marktrolle::Msb, Marktrolle::Nmsb],
        primary_pid: pid(21010),
        dispatch: cmd_wim_gesamtvorgang_melden,
    },
    CommandDescriptor {
        name: "wim.zuordnung.bestaetigen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(21012),
        dispatch: cmd_wim_zuordnung_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.zuordnung.ablehnen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(21011),
        dispatch: cmd_wim_zuordnung_ablehnen,
    },
    // ── WiM Weiterverpflichtung (ORDERS 17002 → ORDRSP 19003/19004) ───────
    //
    // The MSBA's only answer to a Weiterverpflichtungsauftrag. One command,
    // not a bestätigen/ablehnen pair: which of Z13 / Z14 / Z22 applies is a
    // measurement against the cap of Kap. 2.4.2 Nr. 4, and the Cluster the code
    // sits in — not the caller — picks 19003 or 19004.
    CommandDescriptor {
        name: "wim.weiterverpflichtung.beantworten",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(19003),
        dispatch: cmd_wim_weiterverpflichtung_beantworten,
    },
    // ── WiM INSRPT Störungsbehebung (23001 → 23003/23004 → 23008) ─────────
    //
    // The MSB's side of the Use-Case: it answers the inbound Störungsmeldung
    // and, having confirmed it, owes the Ergebnisbericht within a window that
    // depends on the Messtechnik at the Messlokation (WiM Teil 2 Kap. 1.2
    // Nr. 2/7) — which only the MSB's own device registry knows, so the caller
    // supplies it.
    CommandDescriptor {
        name: "wim.stoerung.bestaetigen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(23004),
        dispatch: cmd_wim_stoerung_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.stoerung.ablehnen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(23003),
        dispatch: cmd_wim_stoerung_ablehnen,
    },
    CommandDescriptor {
        name: "wim.stoerung.ergebnis-melden",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(23008),
        dispatch: cmd_wim_stoerung_ergebnis_melden,
    },
    // ── WiM Preisanfrage (REQOTE 35001/35002/35004/35005 → QUOTES 15001/15002/15004/15005) ────────────
    // aMSB answers an inbound REQOTE with the QUOTES Angebot. `processd` M3
    // auto-dispatches this when a current PreisblattMessung exists.
    CommandDescriptor {
        name: "wim.preisanfrage.angebot-senden",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(15001),
        dispatch: cmd_wim_preisanfrage_angebot_senden,
    },
    // ── WiM Steuerungsauftrag ─────────────────────────────────────────────────
    CommandDescriptor {
        name: "wim.steuerungsauftrag.bestaetigen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_steuerungsauftrag_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.steuerungsauftrag.ablehnen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: pid(55039),
        dispatch: cmd_wim_steuerungsauftrag_ablehnen,
    },
    // ── WiM Rechnung (INVOIC 31009 MSB-Rechnung) ──────────────────────────────
    // `invoicd` dispatches these after plausibility check.
    // LF role: payer receives INVOIC from MSB → settle or dispute.
    //
    // The command namespace is German business vocabulary (`rechnung`), while the
    // workflow behind it is named after its EDIFACT message (`wim-invoic`, module
    // `mako_wim::invoic`). That split is deliberate: ERP-facing command names are
    // not renamed when an internal module is. Dispatch fns follow the command
    // name, adapter registries follow the workflow name.
    //
    // **One command per business process.** The same pair settles the
    // MSB-Rechnung 31009, the WiM-Rechnung 31003 and the Sparte-neutral
    // Stornorechnung 31004 — `invoicd` supplies the `invoice_ref`, and the
    // process it resumes already knows its PID.
    //
    // `permitted_roles` is the set of **payers**, read off the PID-Übersicht 4.0
    // „An"-Spalte:
    //
    // | PID | Empfänger | Fundstelle |
    // |---|---|---|
    // | 31009 | NB · LF · ESA | GPKE Teil 3, WiM Strom Teil 1/2, AWH Änd. Technik |
    // | 31003 | NB · MSBN | WiM Strom Teil 1 Kap. 3.7, AWH WiM Gas 2.0 Kap. 4.7 |
    //
    // **`Lfg` is deliberately absent** where `Lf` is present: no WiM-Rechnung
    // ever addresses a Gas-Lieferant. 31009 is a Strom-only Anwendungsfall in
    // every one of its four Festlegungen, and the Gas 31003 goes to the NB and
    // to the incoming MSB — never to the LFG. `Gnb` *is* present, because the
    // Gas NB is a payer of 31003.
    CommandDescriptor {
        name: "wim.rechnung.annehmen",
        permitted_roles: &[
            Marktrolle::Lf,
            Marktrolle::Nb,
            Marktrolle::Gnb,
            Marktrolle::Msb,
            Marktrolle::Nmsb,
            Marktrolle::Esa,
        ],
        primary_pid: pid(31009),
        dispatch: cmd_wim_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "wim.rechnung.ablehnen",
        permitted_roles: &[
            Marktrolle::Lf,
            Marktrolle::Nb,
            Marktrolle::Gnb,
            Marktrolle::Msb,
            Marktrolle::Nmsb,
            Marktrolle::Esa,
        ],
        primary_pid: pid(31009),
        dispatch: cmd_wim_rechnung_ablehnen,
    },
    // The WiM MSB-Wechsel has **no Sparte-specific commands.** AWH WiM Gas 2.0
    // restates WiM Strom Teil 1 use-case for use-case, so `wim.geraetewechsel.*`
    // answers both: the Sparte travels with the process (`wim_sparte(pid)`) and
    // picks the Entscheidungsbaum and the Codeliste without the caller stating
    // anything. Likewise `wim.rechnung.*` settles 31009, 31003 and 31004, and
    // `geli.stornierung.*` owns 44022–44024 for both Use-Cases.
    // PID 31004 Stornorechnung: a single **Sparte-neutral, cross-process** universal
    // Storno (INVOIC AHB §3.1.2 — GPKE/MMM/WiM Strom+Gas/Kapazität/AWH/GeLi). invoicd
    // runs the arithmetic-only check (AcceptedPartial) and dispatches settle/dispute.
    // The receiver can hold any of the storno-facing roles, so permitted_roles spans
    // both Sparten (NB/GNB, LF variants, MSB, BKV, ESA for WiM Strom Teil 2).
    CommandDescriptor {
        name: "invoic.stornorechnung.annehmen",
        permitted_roles: &[
            Marktrolle::Nb,
            Marktrolle::Gnb,
            Marktrolle::Lf,
            Marktrolle::Lfg,
            Marktrolle::Lfn,
            Marktrolle::Lfa,
            Marktrolle::Msb,
            Marktrolle::Bkv,
            Marktrolle::Esa,
        ],
        primary_pid: pid(31004),
        dispatch: cmd_wim_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "invoic.stornorechnung.ablehnen",
        permitted_roles: &[
            Marktrolle::Nb,
            Marktrolle::Gnb,
            Marktrolle::Lf,
            Marktrolle::Lfg,
            Marktrolle::Lfn,
            Marktrolle::Lfa,
            Marktrolle::Msb,
            Marktrolle::Bkv,
            Marktrolle::Esa,
        ],
        primary_pid: pid(31004),
        dispatch: cmd_wim_rechnung_ablehnen,
    },
    // PID 31011 „Rechnung sonstige Leistung": the NB bills the LF for an
    // abrechnungswürdige Handlung — in practice the Sperrung/Entsperrung.
    //
    // **Sparte-neutral**, like the 31004 Storno above: the Anwendungsübersicht
    // Prüfidentifikatoren 4.0 lists 31011 twice, once under GPKE Teil 2
    // („Abrechnung einer sonstigen Leistung", Strom) and once under AWH
    // Sperrprozesse Gas. A Sparte-named command would lock one of the two out on
    // role grounds; the workflow behind it is keyed on `invoice_ref` and carries
    // no Gas semantics.
    //
    // Payload (issuer side): { "invoice_ref": "<rechnungsnummer>",
    //   "sender_mp_id": "<NB MP-ID>", "recipient_mp_id": "<LF MP-ID>",
    //   "pid": 31011, "sparte": "STROM" | "GAS", "rechnung": <BO4E Rechnung JSON> }
    CommandDescriptor {
        name: "invoic.sonstige-leistung.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31011),
        dispatch: cmd_sonstige_leistung_rechnung_stellen,
    },
    CommandDescriptor {
        name: "invoic.sonstige-leistung.annehmen",
        permitted_roles: &[
            Marktrolle::Lf,
            Marktrolle::Lfg,
            Marktrolle::Lfn,
            Marktrolle::Lfa,
        ],
        primary_pid: pid(31011),
        dispatch: cmd_sonstige_leistung_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "invoic.sonstige-leistung.ablehnen",
        permitted_roles: &[
            Marktrolle::Lf,
            Marktrolle::Lfg,
            Marktrolle::Lfn,
            Marktrolle::Lfa,
        ],
        primary_pid: pid(31011),
        dispatch: cmd_sonstige_leistung_rechnung_ablehnen,
    },
    // GaBi Gas MMM-Rechnung (PIDs 31007/31008): NB bills MGV for
    // Mehr-/Mindermengen Gas settlement. BKV/MGV receives and settles/disputes.
    CommandDescriptor {
        name: "gabi.mmm.rechnung.annehmen",
        permitted_roles: &[Marktrolle::Bkv],
        primary_pid: pid(31007),
        dispatch: cmd_gabi_gas_mmm_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "gabi.mmm.rechnung.ablehnen",
        permitted_roles: &[Marktrolle::Bkv],
        primary_pid: pid(31007),
        dispatch: cmd_gabi_gas_mmm_rechnung_ablehnen,
    },
    // ── MABIS Bilanzkreisabrechnung ────────────────────────────────────────────
    CommandDescriptor {
        name: "mabis.abrechnung.einleiten",
        permitted_roles: &[Marktrolle::Bkv],
        primary_pid: pid(13003),
        dispatch: cmd_mabis_abrechnung_einleiten,
    },
    CommandDescriptor {
        name: "mabis.abrechnung.daten-einreichen",
        permitted_roles: &[Marktrolle::Bkv],
        primary_pid: pid(13003),
        dispatch: cmd_mabis_abrechnung_daten_einreichen,
    },
    CommandDescriptor {
        name: "mabis.summenzeitreihe.uebermitteln",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Uenb],
        primary_pid: pid(13003),
        dispatch: cmd_mabis_summenzeitreihe_uebermitteln,
    },
    CommandDescriptor {
        name: "mabis.abrechnung.begleichen",
        permitted_roles: &[Marktrolle::Bkv, Marktrolle::Uenb],
        primary_pid: pid(13003),
        dispatch: cmd_mabis_abrechnung_begleichen,
    },
    // ── IFTSTA status messages (REST replay / manual override) ────────────────
    CommandDescriptor {
        name: "gpke.vollzugsmeldung.empfangen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Lfn, Marktrolle::Lfa],
        primary_pid: None,
        dispatch: cmd_gpke_vollzugsmeldung_empfangen,
    },
    CommandDescriptor {
        name: "wim.iftsta.empfangen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Msb],
        primary_pid: None,
        dispatch: cmd_wim_iftsta_empfangen,
    },
    CommandDescriptor {
        name: "mabis.iftsta.empfangen",
        permitted_roles: &[
            Marktrolle::Bkv,
            Marktrolle::Nb,
            Marktrolle::Uenb,
            Marktrolle::Biko,
        ],
        primary_pid: None,
        dispatch: cmd_mabis_iftsta_empfangen,
    },
    CommandDescriptor {
        name: "mabis.datenstatus.empfangen",
        permitted_roles: &[Marktrolle::Bkv, Marktrolle::Nb, Marktrolle::Biko],
        primary_pid: None,
        dispatch: cmd_mabis_datenstatus_empfangen,
    },
];

/// Resolve and validate the effective Marktrolle for a command.
///
/// Thin boundary wrapper: looks the command up in `COMMAND_REGISTRY`, parses
/// the asserted/configured role strings into typed [`Marktrolle`] values and
/// delegates the licensing policy to [`mako_engine::marktrolle::resolve_role`]
/// (single-permitted → inferred with any assertion ignored; multi-permitted →
/// assertion required + membership; cross-check against the startup
/// `--marktrollen` list, where an empty list rejects everything).
///
/// Returns the resolved Marktrolle code (e.g. `"LF"`) on success.
///
/// # Errors
///
/// See [`CommandError`] — the HTTP/MCP-visible error texts are unchanged.
pub fn validate_command(
    command: &str,
    asserted: Option<&str>,
    configured: &[String],
) -> Result<String, CommandError> {
    let permitted = COMMAND_REGISTRY
        .iter()
        .find(|d| d.name == command)
        .map(|d| d.permitted_roles)
        .ok_or(CommandError::UnknownCommand)?;

    // An asserted code that is not a known Marktrolle can never be in the
    // permitted set. For single-role commands the assertion is ignored anyway;
    // for multi-role commands it is a RoleNotPermitted — same as before typing.
    let asserted_role = match asserted.map(Marktrolle::from_code) {
        None => None,
        Some(Some(r)) => Some(r),
        Some(None) => {
            if permitted.len() == 1 {
                None
            } else {
                return Err(CommandError::RoleNotPermitted);
            }
        }
    };

    // Unknown configured strings are dropped: they could never have matched a
    // permitted BDEW code before typing either.
    let configured_roles: DeploymentRoles = configured
        .iter()
        .filter_map(|s| Marktrolle::from_code(s))
        .collect();

    resolve_role(permitted, asserted_role, &configured_roles)
        .map(|role| role.as_code().to_owned())
        .map_err(|e| match e {
            LicensingError::MarktrolleRequired => CommandError::MarktrolleRequired,
            LicensingError::RoleNotPermitted => CommandError::RoleNotPermitted,
            LicensingError::RoleNotConfigured => CommandError::RoleNotConfigured,
        })
}

// ── Primary PID lookup ────────────────────────────────────────────────────────

/// Returns the primary Prüfidentifikator for a command name.
///
/// Used to populate the Cedar `CommandResource.pid` attribute so that
/// operator ABAC policies can restrict specific API keys to specific PIDs.
/// Returns `None` for unknown commands and for commands that carry no single
/// outbound PID (e.g. REST-only replay sinks or multi-PID ORDERS flows);
/// Cedar call sites map that to the policy-visible `0` sentinel.
///
/// Data is read from [`COMMAND_REGISTRY`] so there is no third parallel data
/// structure to maintain.
pub(crate) fn command_primary_pid(command: &str) -> Option<Pruefidentifikator> {
    COMMAND_REGISTRY
        .iter()
        .find(|d| d.name == command)
        .and_then(|d| d.primary_pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command name a service dispatches over `POST /api/v1/commands`
    /// (the shared list in `mako_markt::commands`) must be registered here.
    ///
    /// The drift guard for the `processd` → `makod` boundary: a name no registry
    /// entry matches is an HTTP 422 on every auto-STP answer it carries.
    #[test]
    fn all_service_dispatched_commands_are_registered() {
        for name in mako_markt::commands::DISPATCHED_BY_SERVICES {
            assert!(
                COMMAND_REGISTRY.iter().any(|d| d.name == *name),
                "command {name:?} is dispatched by a service (mako_markt::commands) \
                 but not registered in COMMAND_REGISTRY"
            );
        }
    }

    /// The service reference's command table must list every registry entry.
    ///
    /// It is what an integrator reads to find a command name; a name that is
    /// only in the code is a command nobody can discover.
    #[test]
    fn every_command_is_in_the_service_reference() {
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../site/content/docs/services/makod.md"
        ))
        .expect("site/content/docs/services/makod.md");
        let missing: Vec<&str> = COMMAND_REGISTRY
            .iter()
            .map(|d| d.name)
            .filter(|n| !doc.contains(&format!("`{n}`")))
            .collect();
        assert!(
            missing.is_empty(),
            "these commands are registered but absent from the \"Command registry\" table \
             in site/content/docs/services/makod.md: {missing:?}"
        );
    }

    /// Registry names must be unique — `dispatch_command` and
    /// `validate_command` both take the first match.
    #[test]
    fn registry_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in COMMAND_REGISTRY {
            assert!(seen.insert(d.name), "duplicate registry entry {:?}", d.name);
        }
    }

    /// PID 31004 Stornorechnung is dispatched under the **Sparte-neutral**
    /// Every outbound INVOIC command is reachable by the role that issues it.
    ///
    /// The role a caller asserts is checked against the deployment's licensed
    /// roles for any command permitted to more than one — so a descriptor that
    /// names the wrong role does not merely mislabel the sender, it locks the
    /// issuing party out of its own invoice. Two cases here are inverted from
    /// the obvious one: PID 31009 is issued by the **MSB**, and the three gas
    /// invoices are issued by a **GNB**.
    #[test]
    fn every_outbound_invoic_permits_the_role_that_issues_it() {
        let roles = |name: &str| {
            COMMAND_REGISTRY
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("{name} must be registered"))
                .permitted_roles
        };

        // PID 31009 — the Messstellenbetreiber is the sender in all seven of
        // its Anwendungsfälle (PID overview 4.0). A `--marktrollen MSB`
        // deployment must be able to send it.
        assert!(
            roles("wim.msb-rechnung.stellen").contains(&Marktrolle::Msb),
            "the MSB issues PID 31009 and must be permitted to send it"
        );

        // The gas invoices: a gas network operator is licensed as GNB.
        for name in [
            "invoic.nne-abschlag.stellen",
            "invoic.nne.stellen",
            "invoic.sonstige-leistung.stellen",
        ] {
            assert!(
                roles(name).contains(&Marktrolle::Gnb),
                "{name} carries a gas invoice and must permit the GNB role"
            );
        }
    }

    /// The storno commands are `invoic.stornorechnung.*`, bound to PID 31004 and
    /// reachable by both Strom (NB) and Gas (GNB) receivers. PID 31004 is
    /// Sparte-neutral (INVOIC AHB §3.1.2), so a `wim.gas.*` name would mislabel
    /// every Strom storno as Gas.
    #[test]
    fn storno_command_is_sparte_neutral() {
        for name in [
            "invoic.stornorechnung.annehmen",
            "invoic.stornorechnung.ablehnen",
        ] {
            let d = COMMAND_REGISTRY
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("{name} must be registered"));
            assert_eq!(d.primary_pid, pid(31004), "{name} must bind PID 31004");
            assert!(
                d.permitted_roles.contains(&Marktrolle::Nb)
                    && d.permitted_roles.contains(&Marktrolle::Gnb),
                "{name} must permit both Strom (Nb) and Gas (Gnb) receivers"
            );
        }
        assert!(
            !COMMAND_REGISTRY
                .iter()
                .any(|d| d.name.starts_with("wim.gas.stornorechnung")),
            "a Gas-only storno command name would mislabel every Strom storno"
        );
    }
}
