//! ERP command names shared between `makod`'s command registry and its clients.
//!
//! `makod` exposes `POST /api/v1/commands` whose `command` field must match an
//! entry in its command registry — an unknown name is rejected with HTTP 422.
//! Services that dispatch commands (`processd`, `invoicd`, …) MUST use these
//! constants instead of string literals so the wire name cannot drift from the
//! registry: `makod` carries a registry test asserting every constant in
//! [`DISPATCHED_BY_SERVICES`] is registered.
//!
//! Only names actually posted by out-of-process callers are listed here; the
//! registry itself remains the single source of truth for roles, PIDs, and
//! dispatch functions.

// ── GPKE (electricity supplier processes) ─────────────────────────────────────

/// LF: initiate a Lieferbeginn Anmeldung (UTILMD 55001).
pub const GPKE_LIEFERBEGINN_ANMELDEN: &str = "gpke.lieferbeginn.anmelden";
/// NB: confirm an inbound Lieferbeginn Anmeldung (UTILMD 55002 / 55078).
pub const GPKE_LIEFERBEGINN_BESTAETIGEN: &str = "gpke.lieferbeginn.bestaetigen";
/// NB: confirm a Neuanlage — inbound 55600 / 55601, answered UTILMD 55602 /
/// 55603 (EBD `E_0608`, Zustimmung `A09` / `A18`).
pub const GPKE_NEUANLAGE_BESTAETIGEN: &str = "gpke.neuanlage.bestaetigen";
/// NB: refuse a Neuanlage — inbound 55600 / 55601, answered UTILMD 55604 /
/// 55605 (EBD `E_0608`).
pub const GPKE_NEUANLAGE_ABLEHNEN: &str = "gpke.neuanlage.ablehnen";
/// NB: assign a contractless `MaLo` to the Grundversorger (UTILMD 55013, §38 `EnWG`).
pub const GPKE_EOG_ANMELDEN: &str = "gpke.eog.anmelden";
/// The E/G supplier confirms an assignment under § 36 / § 38 `EnWG` — 55014,
/// `E_0615`. The Gas twin is [`GELI_EOG_BESTAETIGEN`].
pub const GPKE_EOG_BESTAETIGEN: &str = "gpke.eog.bestaetigen";
/// Refuse one — 55015, `E_0615` `A02` / `A03` / `A04` / `A05` / `A99`.
pub const GPKE_EOG_ABLEHNEN: &str = "gpke.eog.ablehnen";
/// NB: reject an inbound Lieferbeginn Anmeldung (UTILMD 55003 / 55080).
pub const GPKE_LIEFERBEGINN_ABLEHNEN: &str = "gpke.lieferbeginn.ablehnen";
/// LF: initiate a Lieferende Abmeldung (UTILMD 55004).
pub const GPKE_LIEFERENDE_ANMELDEN: &str = "gpke.lieferende.anmelden";
/// LFN: send the Kündigung to the Altlieferant — UTILMD 55016, answered
/// 55017 / 55018 (`E_0614`). The Gas twin is [`GELI_KUENDIGUNG_ANMELDEN`].
pub const GPKE_KUENDIGUNG_ANMELDEN: &str = "gpke.kuendigung.anmelden";
/// NB: confirm an inbound Abmeldung (inbound 55004 → UTILMD 55005, EBD `E_0607`).
pub const GPKE_LIEFERENDE_BESTAETIGEN: &str = "gpke.lieferende.bestaetigen";
/// NB: reject an inbound Abmeldung (inbound 55004 → UTILMD 55006, EBD `E_0607`).
pub const GPKE_LIEFERENDE_ABLEHNEN: &str = "gpke.lieferende.ablehnen";
/// LF: confirm an NB-initiated Lieferende — inbound 55007, answered
/// UTILMD 55008 (EBD `E_0609`).
pub const GPKE_NB_LIEFERENDE_BESTAETIGEN: &str = "gpke.nb-lieferende.bestaetigen";
/// LF: reject an NB-initiated Lieferende — inbound 55007, answered
/// UTILMD 55009 (EBD `E_0609`).
pub const GPKE_NB_LIEFERENDE_ABLEHNEN: &str = "gpke.nb-lieferende.ablehnen";
/// LFA: confirm an NB `Anfrage zur Beendigung der Zuordnung` (UTILMD 55011).
///
/// The inbound PID is 55010 and the EBD is **`E_0624`** ("Anfrage zur Beendigung
/// der Zuordnung prüfen") — distinct from the NB-seitiges Lieferende above
/// (55007 → 55008/55009, EBD `E_0609`).
pub const GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN: &str = "gpke.beendigung-zuordnung.bestaetigen";
/// LFA: reject an NB `Anfrage zur Beendigung der Zuordnung` (UTILMD 55012).
pub const GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN: &str = "gpke.beendigung-zuordnung.ablehnen";

/// NB → LFA: UTILMD 55010 `Anfrage zur Beendigung der Zuordnung`.
///
/// GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 3, „parallel zu Nr. 2": whenever an
/// Anmeldung lands on a Marktlokation that is already assigned at the
/// Zuordnungsbeginn, the NB must ask the incumbent LFA to release it before it
/// may confirm. The LFA answers by **09:00 Uhr des 1. WT**, and silence counts
/// as Zustimmung — so the window closing is a result, not a timeout.
///
/// `E_0623` Prüfschritte 20–50 read that answer, which is why an NB that never
/// sends this cannot reach `A50` „Der LFA hat der Anfrage zur Beendigung der
/// Zuordnung widersprochen" at all.
pub const GPKE_BEENDIGUNG_ZUORDNUNG_ANFRAGEN: &str = "gpke.beendigung-zuordnung.anfragen";

/// NB → LFN: UTILMD 55036 „Information über existierende Zuordnung".
///
/// One of the three **Meldepflichten** around the Lieferbeginn — messages the
/// Festlegung obliges the NB to send with no answer expected back. GPKE Teil 2
/// § 2.1.2 SD Lieferbeginn Nr. 2: owed whenever the Marktlokation is already
/// assigned at the Zuordnungsbeginn, „auch dann …, sofern LFA und LFN identisch
/// sind", and it is what tells the LFN **die Identität des LFA**.
pub const GPKE_ZUORDNUNG_INFORMIEREN: &str = "gpke.zuordnung.informieren";
/// NB → LFA: UTILMD 55037 „Beendigung der Zuordnung" (SD Lieferbeginn Nr. 10).
pub const GPKE_ZUORDNUNG_BEENDEN: &str = "gpke.zuordnung.beenden";
/// NB → LFZ: UTILMD 55038 „Aufhebung einer zukünftigen Zuordnung"
/// (SD Lieferbeginn Nr. 13).
pub const GPKE_ZUORDNUNG_AUFHEBEN: &str = "gpke.zuordnung.aufheben";
/// NB → `MSB` / `MSBZ`: UTILMD 55611 „Beendigung der Zuordnung des `MSB` zur
/// `MaLo` / `MeLo`".
///
/// A Meldepflicht of the SD **Lieferende von NB an LF** (§ 2.5.2 Nr. 11 and
/// Nr. 13), not of the Lieferbeginn: the NB opens that process itself with a
/// 55007, and this tells the `MSB` — or, on `ZH1`, the `MSBZ` — that its own
/// Zuordnung ends. It is the one message in the family that may address a
/// **Messlokation**, because the `MSB` is assigned to the `MeLo` and not to the
/// `MaLo`.
pub const GPKE_MSB_ZUORDNUNG_BEENDEN: &str = "gpke.msb-zuordnung.beenden";

// ── Sperrprozesse (Sperrung / Entsperrung) ───────────────────────────────────

/// LF → NB: order a Sperrung (ORDERS 17115).
pub const GPKE_SPERRUNG_BEAUFTRAGEN: &str = "gpke.sperrung.beauftragen";
/// LF → NB: order the Entsperrung of a sperred Marktlokation (ORDERS 17117).
pub const GPKE_ENTSPERRUNG_BEAUFTRAGEN: &str = "gpke.entsperrung.beauftragen";
/// NB: report a Sperrauftrag as carried out (IFTSTA on the 17115 process).
pub const GPKE_SPERRUNG_BESTAETIGEN: &str = "gpke.sperrung.bestaetigen";
/// NB: report a Sperrauftrag as not carried out (IFTSTA on the 17115 process).
pub const GPKE_SPERRUNG_FEHLGESCHLAGEN: &str = "gpke.sperrung.fehlgeschlagen";

// ── GeLi Gas ──────────────────────────────────────────────────────────────────

/// LF: initiate a gas Lieferbeginn Anmeldung (UTILMD 44001).
pub const GELI_LIEFERBEGINN_ANMELDEN: &str = "geli.lieferbeginn.anmelden";
/// NB: confirm an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_BESTAETIGEN: &str = "geli.lieferbeginn.bestaetigen";
/// NB: reject an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_ABLEHNEN: &str = "geli.lieferbeginn.ablehnen";
/// LF: initiate a gas Lieferende Abmeldung (UTILMD 44004).
pub const GELI_LIEFERENDE_ANMELDEN: &str = "geli.lieferende.anmelden";
/// LFG: send the Kündigung to the Altlieferant — UTILMD G 44016, answered
/// 44017 / 44018 (`E_3001`). BK7-24-01-009 § 3.1; the Strom twin is
/// [`GPKE_KUENDIGUNG_ANMELDEN`].
pub const GELI_KUENDIGUNG_ANMELDEN: &str = "geli.kuendigung.anmelden";
/// GNB: confirm an inbound gas Abmeldung (inbound 44004 → UTILMD 44005).
pub const GELI_LIEFERENDE_BESTAETIGEN: &str = "geli.lieferende.bestaetigen";
/// GNB: reject an inbound gas Abmeldung (inbound 44004 → UTILMD 44006).
pub const GELI_LIEFERENDE_ABLEHNEN: &str = "geli.lieferende.ablehnen";
/// LF: initiate a `GeLi` Gas Stornierung (UTILMD 44022/44023).
pub const GELI_STORNIERUNG_INITIIEREN: &str = "geli.stornierung.initiieren";
/// GNB: assign a contractless Gas `MaLo` to the Ersatzversorger (UTILMD G
/// 44013). The Strom twin is [`GPKE_EOG_ANMELDEN`].
pub const GELI_EOG_ANMELDEN: &str = "geli.eog.anmelden";

// ── WiM Strom ─────────────────────────────────────────────────────────────────

/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// positively. The Anmeldung/Kündigung distinction lives in the spawned
/// `wim-geraetewechsel` process (keyed by `MeLo`), not in the command name.
pub const WIM_GERAETEWECHSEL_BESTAETIGEN: &str = "wim.geraetewechsel.bestaetigen";
/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// negatively (APERAK with reason).
pub const WIM_GERAETEWECHSEL_ABLEHNEN: &str = "wim.geraetewechsel.ablehnen";

/// MSB: answer an inbound Steuerungsauftrag positively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_BESTAETIGEN: &str = "wim.steuerungsauftrag.bestaetigen";
/// MSB: answer an inbound Steuerungsauftrag negatively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_ABLEHNEN: &str = "wim.steuerungsauftrag.ablehnen";
/// aMSB: answer an inbound REQOTE Preisanfrage (35001/35002/35004/35005) with the
/// QUOTES Angebot (15001/15002/15004/15005).
pub const WIM_PREISANFRAGE_ANGEBOT_SENDEN: &str = "wim.preisanfrage.angebot-senden";

// ── ESA Wertebestellung (WiM Kap. 4.1) ───────────────────────────────────────
//
// The MSB side of the ESA subscription: one command per answer the MSB owes,
// and one the ESA sends to end a running subscription.

/// MSB: answer the ESA's Werteanfrage with the QUOTES Angebot (15003).
pub const WIM_WERTEBESTELLUNG_ANBIETEN: &str = "wim.wertebestellung.anbieten";
/// MSB: refuse the ESA's Werteanfrage — the other exit of `E_0252`, carried on
/// the same QUOTES 15003.
pub const WIM_WERTEBESTELLUNG_ANFRAGE_ABLEHNEN: &str = "wim.wertebestellung.anfrage-ablehnen";
/// MSB: answer an inbound Bestellung (ORDERS 17007) with the ORDRSP 19011 /
/// 19012 — one command carrying either cluster's Antwortcode.
pub const WIM_WERTEBESTELLUNG_BESTELLUNG_BEANTWORTEN: &str =
    "wim.wertebestellung.bestellung-beantworten";
/// MSB: answer an inbound Abbestellung (ORDERS 17008) with the ORDRSP
/// 19011 / 19012.
pub const WIM_WERTEBESTELLUNG_ABBESTELLUNG_BEANTWORTEN: &str =
    "wim.wertebestellung.abbestellung-beantworten";
/// MSB: answer an inbound Stornierung der Bestellung (ORDCHG 39002) with the
/// ORDRSP 19013.
pub const WIM_WERTEBESTELLUNG_STORNIERUNG_BEANTWORTEN: &str =
    "wim.wertebestellung.stornierung-beantworten";
/// ESA: end a running Wertebestellung — the ORDERS 17008 Abbestellung, which
/// the MSB answers with [`WIM_WERTEBESTELLUNG_ABBESTELLUNG_BEANTWORTEN`].
pub const ESA_ABBESTELLUNG_BEAUFTRAGEN: &str = "esa.abbestellung.beauftragen";

/// LFN: agree to an announced Zuordnung to an erzeugende Marktlokation or
/// Tranche — inbound 55607, answered UTILMD 55608 (EBDs `E_0603`–`E_0606`).
///
/// The Zustimmung names the Bilanzkreis; without an answer by 15:00 Uhr am ÜT
/// the NB assigns the LFN anyway (GPKE Teil 2 § 2.4.2.2 Nr. 3).
pub const GPKE_ZUORDNUNG_LF_BESTAETIGEN: &str = "gpke.zuordnung-lf.bestaetigen";
/// LFN: refuse an announced Zuordnung — inbound 55607, answered UTILMD 55609.
pub const GPKE_ZUORDNUNG_LF_ABLEHNEN: &str = "gpke.zuordnung-lf.ablehnen";

/// UTILMD 55017 — the LFA agrees to an inbound Kündigung (EBD `E_0614`).
pub const GPKE_KUENDIGUNG_BESTAETIGEN: &str = "gpke.kuendigung.bestaetigen";
/// UTILMD 55018 — the LFA refuses an inbound Kündigung (EBD `E_0614`).
pub const GPKE_KUENDIGUNG_ABLEHNEN: &str = "gpke.kuendigung.ablehnen";

/// UTILMD G 44008 — the LF agrees to an Abmeldung NN vom NB (`E_3002`).
pub const GELI_NB_LIEFERENDE_BESTAETIGEN: &str = "geli.nb-lieferende.bestaetigen";
/// UTILMD G 44009 — the LF refuses an Abmeldung NN vom NB (`E_3002`).
pub const GELI_NB_LIEFERENDE_ABLEHNEN: &str = "geli.nb-lieferende.ablehnen";
/// UTILMD G 44011 — the LFA agrees to an Abmeldeanfrage des NB (`E_3020`).
pub const GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN: &str = "geli.beendigung-zuordnung.bestaetigen";
/// UTILMD G 44012 — the LFA refuses an Abmeldeanfrage des NB (`E_3020`).
pub const GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN: &str = "geli.beendigung-zuordnung.ablehnen";
/// NB → LFN: UTILMD G 44036 „Informationsmeldung über existierende Zuordnung"
/// (AWH `GeLi` Gas V1.2 Kap. 2.5.2 Nr. 2).
pub const GELI_ZUORDNUNG_INFORMIEREN: &str = "geli.zuordnung.informieren";
/// NB → LFA: UTILMD G 44037 „Informationsmeldung zur Beendigung der Zuordnung"
/// (Kap. 2.5.2 Nr. 6, „am selben Tag wie in Prozessschritt 5").
pub const GELI_ZUORDNUNG_BEENDEN: &str = "geli.zuordnung.beenden";
/// NB → LFZ: UTILMD G 44038 „Informationsmeldung zur Aufhebung einer zuk.
/// Zuordnung" (Kap. 2.5.2 Nr. 7).
pub const GELI_ZUORDNUNG_AUFHEBEN: &str = "geli.zuordnung.aufheben";
/// UTILMD G 44017 — the LFA agrees to a Gas Kündigung (`E_3001`).
pub const GELI_KUENDIGUNG_BESTAETIGEN: &str = "geli.kuendigung.bestaetigen";
/// UTILMD G 44018 — the LFA refuses a Gas Kündigung (`E_3001`).
pub const GELI_KUENDIGUNG_ABLEHNEN: &str = "geli.kuendigung.ablehnen";
/// UTILMD G 44014 — the E/G agrees to a Gas EoG-Anmeldung (`E_3008`).
pub const GELI_EOG_BESTAETIGEN: &str = "geli.eog.bestaetigen";
/// UTILMD G 44015 — the E/G refuses a Gas EoG-Anmeldung (`E_3008`).
pub const GELI_EOG_ABLEHNEN: &str = "geli.eog.ablehnen";

// ── INVOIC billing (the answer to an inbound invoice) ─────────────────────────
//
// `invoicd` runs the plausibility check and answers with one of these. Five
// families, one pair each — the whole INVOIC settle/dispute surface, which is
// `mako-invoic`'s single state machine seen from the outside.

/// LF: settle an inbound Netznutzungs- or MMM-Rechnung Strom
/// (PIDs 31001 / 31002 / 31005 / 31006) → REMADV 33001.
pub const GPKE_ABRECHNUNG_ANNEHMEN: &str = "gpke.abrechnung.annehmen";
/// LF: dispute one → REMADV 33002 / 33003 / 33004.
pub const GPKE_ABRECHNUNG_ABLEHNEN: &str = "gpke.abrechnung.ablehnen";

/// Settle an inbound `WiM` invoice — the MSB-Rechnung 31009 and the
/// WiM-Rechnung 31003 (Dienstleistungen im Messwesen, **both Sparten**).
///
/// There is no `wim.gas.*` twin: the Gas 31003 rides this command too, which is
/// why its descriptor carries `Gnb` among the permitted roles.
pub const WIM_RECHNUNG_ANNEHMEN: &str = "wim.rechnung.annehmen";
/// Dispute one, by the Zahlungsziel the invoice carries (`SG8 DTM+265`).
pub const WIM_RECHNUNG_ABLEHNEN: &str = "wim.rechnung.ablehnen";

/// Settle an inbound `GaBi` Gas invoice — the aggregated MMM-Rechnung
/// 31007 / 31008 (received by the **MGV**) and the Kapazitätsrechnung 31010
/// (received by the **BKV**).
pub const GABI_RECHNUNG_ANNEHMEN: &str = "gabi.rechnung.annehmen";
/// Dispute one.
pub const GABI_RECHNUNG_ABLEHNEN: &str = "gabi.rechnung.ablehnen";

/// Settle an inbound Stornorechnung (PID 31004) — Sparte-neutral and
/// cross-process (INVOIC AHB § 3.1.2), so it cancels an invoice from any family.
pub const INVOIC_STORNORECHNUNG_ANNEHMEN: &str = "invoic.stornorechnung.annehmen";
/// Dispute one.
pub const INVOIC_STORNORECHNUNG_ABLEHNEN: &str = "invoic.stornorechnung.ablehnen";

/// Settle an inbound Rechnung sonstige Leistung (PID 31011) — Sparte-neutral
/// (GPKE Teil 2 · AWH Sperrprozesse Gas).
pub const INVOIC_SONSTIGE_LEISTUNG_ANNEHMEN: &str = "invoic.sonstige-leistung.annehmen";
/// Dispute one.
pub const INVOIC_SONSTIGE_LEISTUNG_ABLEHNEN: &str = "invoic.sonstige-leistung.ablehnen";

// ── INVOIC issuance (the invoice itself) ─────────────────────────────────────
//
// The other direction of the same surface: `netzbilanzd` raises the invoice and
// `invoicd` raises the LF's self-billed one. Answering commands sit above.

/// NB / GNB: issue the Abschlagsrechnung Netznutzung (INVOIC 31001).
pub const INVOIC_NNE_ABSCHLAG_STELLEN: &str = "invoic.nne-abschlag.stellen";
/// NB / GNB: issue the Netznutzungsrechnung (INVOIC 31002, both Sparten).
pub const INVOIC_NNE_STELLEN: &str = "invoic.nne.stellen";
/// NB / GNB: issue the Mehr-/Mindermengenrechnung to the LF (INVOIC 31005,
/// both Sparten). The aggregated Gas MMM to the MGV is 31007 / 31008.
pub const INVOIC_MMM_STELLEN: &str = "invoic.mmm.stellen";
/// NB / GNB: issue a Rechnung sonstige Leistung (INVOIC 31011).
pub const INVOIC_SONSTIGE_LEISTUNG_STELLEN: &str = "invoic.sonstige-leistung.stellen";
/// NB / MSB: issue the MSB-Rechnung (INVOIC 31009).
pub const WIM_MSB_RECHNUNG_STELLEN: &str = "wim.msb-rechnung.stellen";
/// LF: issue the self-billed Netznutzungsrechnung (INVOIC 31006,
/// Mehr-/Mindermengen selbst ausgestellt).
pub const GPKE_ABRECHNUNG_SELBSTAUSSTELLEN: &str = "gpke.abrechnung.selbstausstellen";

// ── MaBiS Clearingverfahren ───────────────────────────────────────────────────

/// Answer a `MaBiS` Clearingliste with a **Korrekturliste** — one entry per
/// disputed `Marktlokation`, and an empty list when nothing was found.
///
/// The reply is obligatory either way: silence reads as acceptance of whatever
/// the distributor filed, so „reconciled, nothing to correct" is a message and
/// not an omission (BK6-24-174 Anlage 3, Clearingverfahren).
pub const MABIS_LISTE_KORRIGIEREN: &str = "mabis.liste.korrigieren";
/// Refuse a `MaBiS` Clearingliste **entire** and demand a new one.
///
/// The second, disjoint cluster every Clearinglisten-Tree publishes: it names no
/// `Marktlokation` at all — the Abonnement was never ordered, the version is not
/// admitted, the Zeitraum is implausible, or the list arrived outside the
/// Clearingphase DZÜ. Answering such a list with an empty Korrekturliste would
/// state the opposite of what happened.
pub const MABIS_LISTE_ABLEHNEN: &str = "mabis.liste.ablehnen";
/// NB / ÜNB: submit a `MaBiS` Summenzeitreihe (MSCONS 13003).
pub const MABIS_SUMMENZEITREIHE_UEBERMITTELN: &str = "mabis.summenzeitreihe.uebermitteln";

/// Every command name dispatched by out-of-process services.
///
/// `makod` has a registry test asserting each of these is registered; adding a
/// constant above without registering the command in `makod` fails that test.
/// The reverse — a constant here that no service posts, or a constant above
/// that appears in neither this list nor [`MAKOD_INTERNAL`] — is what
/// `every_constant_is_accounted_for` below refuses.
pub const DISPATCHED_BY_SERVICES: &[&str] = &[
    GPKE_LIEFERBEGINN_ANMELDEN,
    GPKE_EOG_ANMELDEN,
    GPKE_EOG_BESTAETIGEN,
    GPKE_EOG_ABLEHNEN,
    GPKE_LIEFERBEGINN_BESTAETIGEN,
    GPKE_LIEFERBEGINN_ABLEHNEN,
    GPKE_NEUANLAGE_BESTAETIGEN,
    GPKE_NEUANLAGE_ABLEHNEN,
    GPKE_LIEFERENDE_ANMELDEN,
    GPKE_KUENDIGUNG_ANMELDEN,
    GPKE_LIEFERENDE_BESTAETIGEN,
    GPKE_LIEFERENDE_ABLEHNEN,
    GPKE_NB_LIEFERENDE_BESTAETIGEN,
    GPKE_NB_LIEFERENDE_ABLEHNEN,
    GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    GPKE_BEENDIGUNG_ZUORDNUNG_ANFRAGEN,
    GPKE_ZUORDNUNG_INFORMIEREN,
    GPKE_ZUORDNUNG_BEENDEN,
    GPKE_ZUORDNUNG_AUFHEBEN,
    GPKE_MSB_ZUORDNUNG_BEENDEN,
    GPKE_ZUORDNUNG_LF_BESTAETIGEN,
    GPKE_ZUORDNUNG_LF_ABLEHNEN,
    GPKE_KUENDIGUNG_BESTAETIGEN,
    GPKE_KUENDIGUNG_ABLEHNEN,
    GELI_NB_LIEFERENDE_BESTAETIGEN,
    GELI_NB_LIEFERENDE_ABLEHNEN,
    GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    GELI_ZUORDNUNG_INFORMIEREN,
    GELI_ZUORDNUNG_BEENDEN,
    GELI_ZUORDNUNG_AUFHEBEN,
    GELI_KUENDIGUNG_BESTAETIGEN,
    GELI_KUENDIGUNG_ABLEHNEN,
    GELI_EOG_BESTAETIGEN,
    GELI_EOG_ABLEHNEN,
    GELI_LIEFERBEGINN_ANMELDEN,
    GELI_LIEFERBEGINN_BESTAETIGEN,
    GELI_LIEFERBEGINN_ABLEHNEN,
    GELI_LIEFERENDE_ANMELDEN,
    GELI_KUENDIGUNG_ANMELDEN,
    GELI_LIEFERENDE_BESTAETIGEN,
    GELI_LIEFERENDE_ABLEHNEN,
    GELI_STORNIERUNG_INITIIEREN,
    WIM_GERAETEWECHSEL_BESTAETIGEN,
    WIM_GERAETEWECHSEL_ABLEHNEN,
    WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
    WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
    WIM_PREISANFRAGE_ANGEBOT_SENDEN,
    GPKE_ABRECHNUNG_ANNEHMEN,
    GPKE_ABRECHNUNG_ABLEHNEN,
    WIM_RECHNUNG_ANNEHMEN,
    WIM_RECHNUNG_ABLEHNEN,
    GABI_RECHNUNG_ANNEHMEN,
    GABI_RECHNUNG_ABLEHNEN,
    INVOIC_STORNORECHNUNG_ANNEHMEN,
    INVOIC_STORNORECHNUNG_ABLEHNEN,
    INVOIC_SONSTIGE_LEISTUNG_ANNEHMEN,
    INVOIC_SONSTIGE_LEISTUNG_ABLEHNEN,
    MABIS_LISTE_KORRIGIEREN,
    MABIS_LISTE_ABLEHNEN,
    MABIS_SUMMENZEITREIHE_UEBERMITTELN,
    GPKE_SPERRUNG_BEAUFTRAGEN,
    GPKE_ENTSPERRUNG_BEAUFTRAGEN,
    GPKE_SPERRUNG_BESTAETIGEN,
    GPKE_SPERRUNG_FEHLGESCHLAGEN,
    GELI_EOG_ANMELDEN,
    WIM_WERTEBESTELLUNG_ANBIETEN,
    WIM_WERTEBESTELLUNG_ANFRAGE_ABLEHNEN,
    WIM_WERTEBESTELLUNG_BESTELLUNG_BEANTWORTEN,
    WIM_WERTEBESTELLUNG_ABBESTELLUNG_BEANTWORTEN,
    WIM_WERTEBESTELLUNG_STORNIERUNG_BEANTWORTEN,
    ESA_ABBESTELLUNG_BEAUFTRAGEN,
    INVOIC_NNE_ABSCHLAG_STELLEN,
    INVOIC_NNE_STELLEN,
    INVOIC_MMM_STELLEN,
    INVOIC_SONSTIGE_LEISTUNG_STELLEN,
    WIM_MSB_RECHNUNG_STELLEN,
    GPKE_ABRECHNUNG_SELBSTAUSSTELLEN,
];

/// Constants `makod` itself needs but no out-of-process caller posts, with why.
///
/// Empty, and worth keeping that way. This module's whole claim is that it
/// holds "only names actually posted by out-of-process callers"; a constant in
/// neither list is a claim nothing backs — four sat here unused, each the wire
/// name of a command that exists in `makod`'s registry and that no service ever
/// named, so the export bought a second spelling and nothing else. An entry
/// here has to argue in writing why a name belongs in the shared catalogue
/// while living entirely inside `makod`.
pub const MAKOD_INTERNAL: &[(&str, &str)] = &[];

#[cfg(test)]
mod tests {
    use super::{DISPATCHED_BY_SERVICES, MAKOD_INTERNAL};

    /// Every `&str` constant this module exports is accounted for.
    ///
    /// The forward direction — every listed name is one `makod` registers — is
    /// asserted by `makod`'s registry test, and `cargo xtask
    /// check-answer-commands` asserts that every service names a constant
    /// rather than a literal. Neither looks at a constant that is simply *not*
    /// listed: it compiles, it exports, and it is a second spelling of a wire
    /// name with nothing holding it to the registry.
    ///
    /// So a `pub const … : &str` here must appear in [`DISPATCHED_BY_SERVICES`]
    /// or in [`MAKOD_INTERNAL`] with a reason. Read out of this file's own
    /// text, because the set of exports is not a value the compiler hands us.
    #[test]
    fn every_constant_is_accounted_for() {
        let src = include_str!("commands.rs");
        let listed: Vec<&str> = listed_names(src);
        let internal: Vec<&str> = MAKOD_INTERNAL.iter().map(|(n, _)| *n).collect();

        let mut orphans = Vec::new();
        for name in exported_str_constants(src) {
            if !listed.contains(&name.as_str()) && !internal.contains(&name.as_str()) {
                orphans.push(name);
            }
        }
        assert!(
            orphans.is_empty(),
            "{} command constant(s) in neither DISPATCHED_BY_SERVICES nor MAKOD_INTERNAL: \
             {}.\n  A constant no service posts is a second spelling of a wire name that \
             nothing holds to makod's registry — delete it, or list it in MAKOD_INTERNAL \
             with the reason it belongs in the shared catalogue.",
            orphans.len(),
            orphans.join(", ")
        );

        // The list is read from text; if the reader stops finding entries the
        // assertion above passes vacuously.
        assert!(
            listed.len() == DISPATCHED_BY_SERVICES.len() && listed.len() > 60,
            "the DISPATCHED_BY_SERVICES reader found {} of {} entries",
            listed.len(),
            DISPATCHED_BY_SERVICES.len()
        );
    }

    /// The names of every `pub const NAME: &str = "…";` in `src`.
    fn exported_str_constants(src: &str) -> Vec<String> {
        src.lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("pub const ")?;
                let (name, tail) = rest.split_once(':')?;
                // `&[&str]` and `&[(&str, &str)]` are the two lists, not names.
                tail.trim_start()
                    .starts_with("&str")
                    .then(|| name.trim().to_owned())
            })
            .collect()
    }

    /// The constant names spelled inside the `DISPATCHED_BY_SERVICES` literal.
    fn listed_names(src: &str) -> Vec<&str> {
        let start = src
            .find("pub const DISPATCHED_BY_SERVICES")
            .expect("the list is declared here");
        let open = start + src[start..].find("= &[").expect("the list opens") + 4;
        let close = open + src[open..].find(']').expect("the list closes");
        src[open..close]
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .collect()
    }

    /// The adversarial case: a constant listed nowhere must fail.
    #[test]
    fn an_unlisted_constant_is_refused() {
        // Built by `concat!` so no line of *this* file starts with
        // `pub const` — the reader below runs over this file too.
        let src = concat!(
            "pub const A: &str = \"gpke.a.b\";\n",
            "pub const B: &str = \"gpke.c.d\";\n",
            "pub const DISPATCHED_BY_SERVICES: &[&str] = &[\n    A,\n];\n",
        );
        assert_eq!(
            exported_str_constants(src),
            vec!["A".to_owned(), "B".to_owned()]
        );
        assert_eq!(listed_names(src), vec!["A"]);
    }
}
