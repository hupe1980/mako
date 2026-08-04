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
// - BDEW WiM AHB (BK6-18-032)
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
        primary_pid: pid(55002),
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
    // ── GPKE Kündigung ────────────────────────────────────────────────────────
    CommandDescriptor {
        name: "gpke.kuendigung.anmelden",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(55016),
        dispatch: cmd_gpke_kuendigung_anmelden,
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
    // Payload: { "invoice_ref": "<uuid>", "nb_mp_id": "<GLN>", "lf_mp_id": "<GLN>",
    //            "pid": <PID>, "rechnung": <BO4E Rechnung JSON> }
    CommandDescriptor {
        name: "gpke.nne.rechnung.stellen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(31002),
        dispatch: cmd_gpke_nne_rechnung_stellen,
    },
    CommandDescriptor {
        name: "gpke.mmm.rechnung.stellen",
        permitted_roles: &[Marktrolle::Nb],
        primary_pid: pid(31005),
        dispatch: cmd_gpke_mmm_rechnung_stellen,
    },
    CommandDescriptor {
        name: "gpke.nne-gas.rechnung.stellen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31002),
        dispatch: cmd_gpke_nne_gas_rechnung_stellen,
    },
    CommandDescriptor {
        name: "wim.msb-rechnung.stellen",
        permitted_roles: &[Marktrolle::Nb],
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
        primary_pid: pid(44003),
        dispatch: cmd_geli_lieferbeginn_bestaetigen,
    },
    CommandDescriptor {
        name: "geli.lieferbeginn.ablehnen",
        permitted_roles: &[Marktrolle::Gnb],
        primary_pid: pid(44004),
        dispatch: cmd_geli_lieferbeginn_ablehnen,
    },
    // ── GeLi Gas Lieferende (gas) ─────────────────────────────────────────────
    CommandDescriptor {
        name: "geli.lieferende.anmelden",
        permitted_roles: &[Marktrolle::Lfg],
        primary_pid: pid(44002),
        dispatch: cmd_geli_lieferende_anmelden,
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
    // ── GeLi Gas LF Stornierung (ERP-initiated, LF sends 44022 to GNB) ────────
    // LFN/LFA initiates a supply-change cancellation; ERP supplies `malo_id`
    // and optional `bgm_qualifier` (E01=Kündigung, E02=Rücktritt, E35=Sperrung).
    CommandDescriptor {
        name: "geli.gas.stornierung.initiieren",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(44022),
        dispatch: cmd_geli_gas_stornierung_initiieren,
    },
    // ── GeLi Gas Datenabruf (ORDERS 17103 — LF requests Brennwert/Zustandszahl) ─
    // LF sends ORDERS 17103 outbound to NB/MSB requesting Gas quality data.
    // Spawns a GeliGasDatanabrufWorkflow that tracks the 10-Werktage response.
    CommandDescriptor {
        name: "geli.gas.datenabruf.anfragen",
        permitted_roles: &[Marktrolle::Lf],
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
        name: "wim.wertebestellung.abbestellung-bestaetigen",
        permitted_roles: &[Marktrolle::Msb],
        primary_pid: Some(mako_wim::wertebestellung::BESTAETIGUNG_PID),
        dispatch: cmd_wim_wertebestellung_abbestellung_bestaetigen,
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
    // ── WiM Preisanfrage (REQOTE 35001–35005 → QUOTES 15001–15005) ────────────
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
    // `mako_wim::invoic`). That split is deliberate and mirrored on the Gas side
    // (`wim.gas.rechnung.*` → `wim-gas-invoic`): ERP-facing command names are not
    // renamed when an internal module is. Dispatch fns follow the command name,
    // adapter registries follow the workflow name.
    CommandDescriptor {
        name: "wim.rechnung.annehmen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(31009),
        dispatch: cmd_wim_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "wim.rechnung.ablehnen",
        permitted_roles: &[Marktrolle::Lf],
        primary_pid: pid(31009),
        dispatch: cmd_wim_rechnung_ablehnen,
    },
    // ── WiM Gas MSB-Wechsel (UTILMD G, GNB side) ─────────────────────────────
    // The GNB receives inbound UTILMD G messages from the nMSBG via AS4 and
    // must respond with an APERAK within 10 Werktage (BK7-24-01-009).
    // These commands let the ERP (or processd auto-STP) dispatch the response.
    //
    // Payload: { "malo_id": "<11-digit gas MaLo>" }
    // Optional: { "reason": "<rejection reason>" } for ablehnen variants.
    CommandDescriptor {
        name: "wim.gas.anmeldung.bestaetigen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44042),
        dispatch: cmd_wim_gas_anmeldung_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.gas.anmeldung.ablehnen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44042),
        dispatch: cmd_wim_gas_anmeldung_ablehnen,
    },
    // WiM Gas Kündigung: nMSBG sends UTILMD G Kündigung (PIDs 44039–44041) to GNB.
    // Payload: { "malo_id": "<gas MaLo>" }
    CommandDescriptor {
        name: "wim.gas.kuendigung.bestaetigen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44039),
        dispatch: cmd_wim_gas_kuendigung_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.gas.kuendigung.ablehnen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44039),
        dispatch: cmd_wim_gas_kuendigung_ablehnen,
    },
    // WiM Gas Stornierung: LFN/LFA sends PID 44022 to GNB; GNB responds with
    // 44023 (positive) or 44024 (negative). Business key = vorgang_id from IDE+24.
    // Payload: { "vorgang_id": "<Vorgangsnummer from PID 44022>" }
    CommandDescriptor {
        name: "wim.gas.stornierung.bestaetigen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44022),
        dispatch: cmd_wim_gas_stornierung_bestaetigen,
    },
    CommandDescriptor {
        name: "wim.gas.stornierung.ablehnen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(44022),
        dispatch: cmd_wim_gas_stornierung_ablehnen,
    },
    // ── Gas INVOIC settlement / dispute ────────────────────────────────────────
    // These commands are dispatched by `invoicd` after the automated plausibility
    // check (invoic-checker 6-check pipeline) completes. The business key is the
    // EDIFACT message-reference (`invoice_ref`) from the original INVOIC message.
    //
    // They must exist in the registry so that Cedar ABAC permission checks and
    // `list_commands` correctly reflect what `invoicd` dispatches to `makod`.
    //
    // Payload: { "invoice_ref": "<EDIFACT UNH message reference>" }
    // Optional: { "ablehnungsgrund": "<reason>" } for ablehnen variants.
    CommandDescriptor {
        name: "wim.gas.rechnung.annehmen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31003),
        dispatch: cmd_wim_gas_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "wim.gas.rechnung.ablehnen",
        permitted_roles: &[Marktrolle::Nb, Marktrolle::Gnb],
        primary_pid: pid(31003),
        dispatch: cmd_wim_gas_rechnung_ablehnen,
    },
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
        dispatch: cmd_invoic_stornorechnung_annehmen,
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
        dispatch: cmd_invoic_stornorechnung_ablehnen,
    },
    // GeLi Gas AWH Sperrprozesse INVOIC (PID 31011): VNB bills LFN/LFA for services
    // rendered during the gas disconnection/reconnection process.
    CommandDescriptor {
        name: "geli.gas.rechnung.annehmen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Lfg],
        primary_pid: pid(31011),
        dispatch: cmd_geli_gas_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "geli.gas.rechnung.ablehnen",
        permitted_roles: &[Marktrolle::Lf, Marktrolle::Lfg],
        primary_pid: pid(31011),
        dispatch: cmd_geli_gas_rechnung_ablehnen,
    },
    // GaBi Gas MMM-Rechnung (PIDs 31007/31008): NB bills MGV for
    // Mehr-/Mindermengen Gas settlement. BKV/MGV receives and settles/disputes.
    CommandDescriptor {
        name: "gabi.gas.mmm.rechnung.annehmen",
        permitted_roles: &[Marktrolle::Bkv],
        primary_pid: pid(31007),
        dispatch: cmd_gabi_gas_mmm_rechnung_annehmen,
    },
    CommandDescriptor {
        name: "gabi.gas.mmm.rechnung.ablehnen",
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
    /// This is the drift guard for the `processd` → `makod` boundary: `processd`
    /// once posted `wim.msb-wechsel.*` / `geli.gas.lieferbeginn.*` names that no
    /// registry entry matched, so every auto-STP answer died with HTTP 422.
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
    /// `invoic.stornorechnung.*` names (not the retired Gas-only
    /// `wim.gas.stornorechnung.*`), bound to PID 31004, and reachable by both
    /// Strom (NB) and Gas (GNB) receivers — so a Strom storno is no longer
    /// mislabelled Gas. Regression guard for INVOIC AHB §3.1.2.
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
        // The Gas-only names are gone (hard cut — no backward compatibility).
        assert!(
            !COMMAND_REGISTRY
                .iter()
                .any(|d| d.name.starts_with("wim.gas.stornorechnung")),
            "retired Gas-only storno command names must not linger"
        );
    }
}
