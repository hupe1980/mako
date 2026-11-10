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
/// Berechtigter: answer an inbound Gas Stammdatenänderung — the 44115 to the
/// NB's 44112 / 44113, the 44111 / 44121 / 44124 / 44119 / 44161 / 44176 to
/// the others — with a code of the Antwort column's Codeliste (UTILMD AHB Gas
/// 1.2 Kap. 5.10–5.11). Body: [`GeliStammdatenAenderungBeantworten`].
pub const GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN: &str = "geli.stammdaten-aenderung.beantworten";
/// Verantwortlicher: send a nicht bilanzierungsrelevante Gas Stammdatenänderung
/// — the LF's 44109 (Kap. 5.11.3) or the NB's 44112 (Kap. 5.10.1). Body:
/// [`GeliStammdatenAenderungSenden`].
pub const GELI_STAMMDATEN_AENDERUNG_SENDEN: &str = "geli.stammdaten-aenderung.senden";

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
/// NB / GNB: issue the Mehr-/Mindermengenrechnung to the LF, both Sparten —
/// the Mindermenge as 31005, the Mehrmenge as the selbst ausgestellte 31006
/// (INVOIC AHB 1.0b Kap. 3, S. 5). The aggregated Gas MMM to the MGV is
/// 31007 / 31008.
pub const INVOIC_MMM_STELLEN: &str = "invoic.mmm.stellen";
/// NB / GNB: issue a Rechnung sonstige Leistung (INVOIC 31011).
pub const INVOIC_SONSTIGE_LEISTUNG_STELLEN: &str = "invoic.sonstige-leistung.stellen";
/// NB / MSB: issue the MSB-Rechnung (INVOIC 31009).
pub const WIM_MSB_RECHNUNG_STELLEN: &str = "wim.msb-rechnung.stellen";

// ── The MSB's answers to the ORDERS it receives ───────────────────────────────

/// MSB Strom: answer the NB's Anfrage Sperrung 17116 with ORDRSP 19118 / 19119
/// out of `E_0488`.
pub const GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN: &str = "gpke.anfrage-sperrung.beantworten";
/// gMSB: answer the GNB's Anfrage Sperrung 17116 with ORDRSP 19118 / 19119 out
/// of `E_1001`.
pub const GELI_ANFRAGE_SPERRUNG_BEANTWORTEN: &str = "geli.anfrage-sperrung.beantworten";
/// MSB: answer a received ORDERS 17011 Änderung der Technik with ORDRSP 19005
/// / 19006 out of `E_0249` / `E_0250` / `E_0279` / `E_0283`.
pub const WIM_TECHNIK_AENDERUNG_BEANTWORTEN: &str = "wim.technik-aenderung.beantworten";
/// MSB: refuse an Anfrage / Anforderung / Reklamation von Werten (ORDERS
/// 17102 / 17004 / 17113) with ORDRSP 19102 / 19007 / 19114.
pub const GPKE_DATENABRUF_ABLEHNEN: &str = "gpke.datenabruf.ablehnen";
/// MSB: answer a received GPKE Teil 3 ORDERS (17130, 17121, 17118, 17129) with
/// ORDRSP 19132 / 19120 / 19127 / 19131.
pub const GPKE_KONFIGURATION_BEANTWORTEN: &str = "gpke.konfiguration.beantworten";
/// LF / NB / MSB: send an Anforderung von Werten (ORDERS 17004) to the MSB.
pub const GPKE_DATENABRUF_ANFORDERN: &str = "gpke.datenabruf.anfordern";
/// MSBN: announce a Gerätewechselabsicht (ORDERS 17009) to the MSBA.
pub const WIM_GERAETEWECHSELABSICHT_ANZEIGEN: &str = "wim.geraetewechselabsicht.anzeigen";
/// MSBN: order the offered devices (ORDERS 17001) from the MSBA.
pub const WIM_GERAETEUEBERNAHME_BESTELLEN: &str = "wim.geraeteuebernahme.bestellen";

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
    GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN,
    GELI_STAMMDATEN_AENDERUNG_SENDEN,
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
    GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN,
    GELI_ANFRAGE_SPERRUNG_BEANTWORTEN,
    WIM_TECHNIK_AENDERUNG_BEANTWORTEN,
    GPKE_DATENABRUF_ABLEHNEN,
    GPKE_KONFIGURATION_BEANTWORTEN,
    GPKE_DATENABRUF_ANFORDERN,
    WIM_GERAETEWECHSELABSICHT_ANZEIGEN,
    WIM_GERAETEUEBERNAHME_BESTELLEN,
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

// ── Command bodies ────────────────────────────────────────────────────────────

/// The `payload` of [`GPKE_EOG_ANMELDEN`] — the NB's Anmeldung `EoG` 55013.
///
/// One type on both sides: `makod` reads the payload as this type and refuses
/// a key it does not declare, and a dispatching service builds the payload
/// from it, so the two cannot disagree on a field. The Marktlokation's
/// Stammdaten are not part of it: `makod` reads them from `marktd` at its
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeEogAnmelden {
    /// The Marktlokation.
    pub malo_id: String,
    /// MP-ID of the Grundversorger (§ 36 Abs. 2 `EnWG` Feststellung).
    pub gv_mp_id: String,
    /// Zuordnungsbeginn `SG4 DTM+92`, `YYYY-MM-DD` or `YYYYMMDD` (may be
    /// retroactive).
    pub process_date: String,
    /// `SG4 STS+7` DE 9013 Transaktionsgrund.
    pub transaktionsgrund: String,
}

/// The `payload` of [`GELI_EOG_ANMELDEN`] — the GNB's Anmeldung `EoG` 44013.
/// See [`GpkeEogAnmelden`] for why it is shared.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliEogAnmelden {
    /// The Marktlokation.
    pub malo_id: String,
    /// MP-ID of the E/G Lieferant.
    pub gv_mp_id: String,
    /// Zuordnungsbeginn `SG4 DTM+92`, `YYYY-MM-DD` or `YYYYMMDD`.
    pub process_date: String,
    /// `SG4 DTM+158` Bilanzierungsbeginn, `YYYY-MM-DD` or `YYYYMMDD`.
    pub bilanzierungsbeginn: String,
    /// `SG4 STS+7` DE 9013 Transaktionsgrund.
    pub transaktionsgrund: String,
}

/// The Abrechnungsintervalle des LF a 44001 or 44014 may state in `SG6
/// DTM+Z20` DE 2380, in Monaten: monatlich, vierteljährlich, halbjährlich,
/// jährlich — the four entries UTILMD MIG Gas G1.2 S. 55 admits.
pub const ABRECHNUNGSINTERVALLE_MONATE: [u8; 4] = [1, 3, 6, 12];

/// `SG8 SEQ+Z75 SG10 CCI+Z61` DE 7037 „Grundlage zur Verringerung der Umlagen
/// nach `EnFG`" on a Bestätigung `EoG` 55014, with the AHB's wording (UTILMD AHB
/// Strom 2.2 Kap. 8.6, S. 138). A fact about the Kunde, which the E/G states.
pub const ENFG_GRUNDLAGEN: [(&str, &str); 3] = [
    ("ZF9", "Kunde erfüllt die Voraussetzung nach EnFG"),
    ("ZG0", "Kunde erfüllt nicht die Voraussetzung nach EnFG"),
    (
        "ZG1",
        "Keine Angabe, da Marktlokation die Voraussetzung zur Verringerung der Umlagen nach \
         EnFG nicht erfüllt",
    ),
];

/// Whether `grundlage` is the [`ENFG_GRUNDLAGEN`] code beside which the Grund
/// der Privilegierung is Muss — `ZF9`, „Wenn in derselben SG10 das
/// CCI+Z61++ZF9 … vorhanden" (Bedingung `[463]`, S. 139).
#[must_use]
pub fn enfg_mit_grund(grundlage: &str) -> bool {
    grundlage == "ZF9"
}

/// `SG10 CAV` DE 7111 „Grund der Privilegierung nach `EnFG`" — Muss beside
/// `ZF9` ([`enfg_mit_grund`]) and listed under no other code — with the AHB's wording
/// (UTILMD AHB Strom 2.2 Kap. 8.6, S. 139).
pub const ENFG_PRIVILEGIERUNGEN: [(&str, &str); 9] = [
    ("ZU5", "§ 21 EnFG Stromspeicher und Verlustenergie"),
    ("ZU6", "§ 22 EnFG elektrisch angetriebene Wärmepumpen"),
    (
        "ZU7",
        "§ 23 EnFG Umlageerhebung bei Anlagen zur Verstromung von Kuppelgasen",
    ),
    ("ZU8", "§ 24 EnFG Herstellung von Grünen Wasserstoff"),
    ("ZU9", "§§ 30 - 35 EnFG stromkostenintensive Unternehmen"),
    (
        "ZV0",
        "§ 36 EnFG Herstellung von Wasserstoff in stromkostenintensiven Unternehmen",
    ),
    ("ZV1", "§ 37 EnFG Schienenbahnen"),
    (
        "ZV2",
        "§ 38 EnFG elektrische betriebene Bussen im Linienverkehr",
    ),
    ("ZV3", "§ 39 EnFG Landstromanlagen"),
];

/// The body of [`GPKE_LIEFERBEGINN_ANMELDEN`] — the Anmeldung verbrauchende
/// Marktlokation 55001, LF → NB (UTILMD AHB Strom 2.2 Kap. 8.2, S. 70–87). See
/// [`GpkeEogAnmelden`] for why it is shared.
///
/// Every field but the Ergänzung is Muss in the column, and each is a fact of
/// the Lieferant's contract with its Kunde. The door holds the rendered 55001
/// against its profile before anything is written.
///
/// | Field | Segment | AHB S. |
/// |---|---|---|
/// | `transaktionsgrund` | `SG4 STS+7` DE 9013, `E01` Ein-/Auszug (Umzug) or `E03` Wechsel | 73 |
/// | `bilanzkreis` | `SG8 SEQ+Z79` Produktpaket (Kap. 5.3) | 78 |
/// | `haushaltskunde` | `SG8 SEQ+Z01` `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung (nach EnWG) | 83 |
/// | `enfg_grundlage`, `enfg_grund` | `SG8 SEQ+Z75` `SG10 CCI+Z61`, `CAV` beside `ZF9` | 85 |
/// | `kunde` | `SG12 NAD+Z09` Kunde des Lieferanten, `NAD+Z04` Korrespondenzanschrift | 86 |
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeLieferbeginnAnmelden {
    /// The Marktlokation — `SG5 LOC+Z16`.
    pub malo_id: String,
    /// Lieferbeginn `SG4 DTM+92`, `YYYY-MM-DD`.
    pub lieferbeginn_datum: String,
    /// `SG4 STS+7` DE 9013 — `E01` or `E03`.
    pub transaktionsgrund: String,
    /// `SG4 STS+7` DE 9013 Ergänzung — `ZW4` verbrauchende or `ZAP` ruhende
    /// Marktlokation; `ZW4` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG8 SEQ+Z79` / `SG10 CAV+ZV4` — the Bilanzkreis, an X- or Y-EIC.
    pub bilanzkreis: String,
    /// `true` for `CCI+++Z15`, a Haushaltskunde; `false` for `Z18`.
    pub haushaltskunde: bool,
    /// `CCI+Z61` — one of the [`ENFG_GRUNDLAGEN`] codes.
    pub enfg_grundlage: String,
    /// `CAV` — one of the [`ENFG_PRIVILEGIERUNGEN`] codes, beside `ZF9` and
    /// nowhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enfg_grund: Option<String>,
    /// The Kunde des Lieferanten with his Korrespondenzanschrift — or, where
    /// he has none, the Anschrift der Marktlokation (Bedingung `[556]`).
    pub kunde: crate::utilmd_stammdaten::Kunde,
}

impl GpkeLieferbeginnAnmelden {
    /// Every value the 55001 column cannot carry, each naming its segment —
    /// empty when the body is sendable.
    ///
    /// The Transaktionsgrund is not checked here: the workflow that writes the
    /// message holds it against the column's codes.
    #[must_use]
    pub fn maengel(&self) -> Vec<String> {
        let mut m = Vec::new();
        for (field, value) in [
            ("malo_id (SG5 LOC+Z16)", &self.malo_id),
            ("lieferbeginn_datum (SG4 DTM+92)", &self.lieferbeginn_datum),
            ("bilanzkreis (SG8 SEQ+Z79)", &self.bilanzkreis),
        ] {
            if value.trim().is_empty() {
                m.push(format!("{field} is empty"));
            }
        }
        if !ENFG_GRUNDLAGEN
            .iter()
            .any(|(code, _)| *code == self.enfg_grundlage)
        {
            m.push(format!(
                "enfg_grundlage {:?} (SG8 SEQ+Z75 CCI+Z61): one of ZF9, ZG0, ZG1",
                self.enfg_grundlage
            ));
        }
        match (
            enfg_mit_grund(&self.enfg_grundlage),
            self.enfg_grund.as_deref(),
        ) {
            (true, Some(g)) if ENFG_PRIVILEGIERUNGEN.iter().any(|(c, _)| *c == g) => {}
            (true, _) => m.push(
                "enfg_grund (SG10 CAV): Muss beside ZF9 — one of ZU5 … ZV3 (Bedingung [463])"
                    .to_owned(),
            ),
            (false, Some(g)) => m.push(format!(
                "enfg_grund {g:?} (SG10 CAV) is listed beside ZF9 only"
            )),
            (false, None) => {}
        }
        m.extend(
            self.kunde
                .anmeldung_maengel()
                .into_iter()
                .map(|f| format!("kunde (SG12 NAD+Z09/Z04): {f}")),
        );
        m
    }
}

/// The `payload` of [`GELI_LIEFERBEGINN_ANMELDEN`] — the LF's Anmeldung NN
/// **44001** (UTILMD AHB Gas 1.2 Kap. 5.6, S. 43–66). See [`GpkeEogAnmelden`]
/// for why it is shared.
///
/// Every field is a fact of the Lieferant's contract with its Kunde, which is
/// why `vertragd` states them and `processd` forwards them: the column makes
/// each one Muss, and none is a record of the Netzbetreiber's.
///
/// | Field | Segment | AHB S. |
/// |---|---|---|
/// | `transaktionsgrund` | `SG4 STS+7` DE 9013 | 45 |
/// | `abrechnungsintervall_monate` | `SG6 RFF+Z18` `DTM+Z20` | 48 |
/// | `bilanzkreis` | `SG10 CCI+Z19` | 50 |
/// | `haushaltskunde` | `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung (nach EnWG) | 51 |
/// | `netznutzungsvertrag`, `netznutzung_zahler` | `SG10 CCI+++Z88`, `CAV+Z74`, `CAV+Z73` | 52 |
/// | `gemeinderabatt_prozent` | `SG8 SEQ+Z12` `QTY+Z16` | 55 |
/// | `kunde` | `SG12 NAD+Z09` Kunde des LF, `NAD+Z04` Korrespondenzanschrift | 63–64 |
/// | `ablesekarte` | `SG12 NAD+Z05` Name und Adresse für die Ablesekarte | 65 |
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliLieferbeginnAnmelden {
    /// The Marktlokation — `SG5 LOC+172` under `IMD++Z36+Z12`.
    pub malo_id: String,
    /// Lieferbeginn `SG4 DTM+92`, `YYYYMMDD` (gesetzliche deutsche Zeit).
    pub process_date: String,
    /// `SG4 STS+7` DE 9013 — one of the codes the 44001 column admits.
    pub transaktionsgrund: String,
    /// `SG10 CCI+Z19` DE 7037.
    pub bilanzkreis: String,
    /// `SG6 DTM+Z20` „Abrechnungsintervall des LF", one of
    /// [`ABRECHNUNGSINTERVALLE_MONATE`].
    pub abrechnungsintervall_monate: u8,
    /// `true` for `CCI+++Z15`, a Haushaltskunde; `false` for `Z18`.
    pub haushaltskunde: bool,
    /// `CAV+Z74` — the Netznutzungsvertrag with the NB.
    pub netznutzungsvertrag: crate::utilmd_stammdaten::Netznutzungsvertrag,
    /// `CAV+Z73` — who pays the Netznutzung. The Kunde only under a
    /// direkter Vertrag zwischen Kunden und NB (Bedingung `[216]`).
    pub netznutzung_zahler: crate::utilmd_stammdaten::NetznutzungZahler,
    /// `QTY+Z16` in Prozent: `≥ 0` (`[902]`), at most two Nachkommastellen
    /// (`[930]`), `≤ 10` (`[938]`).
    pub gemeinderabatt_prozent: rust_decimal::Decimal,
    /// The Kunde des Lieferanten, with his Korrespondenzanschrift — or, where
    /// he has none, the Anschrift der Marktlokation (Bedingung `[556]`).
    pub kunde: crate::utilmd_stammdaten::Kunde,
    /// Name und Adresse für die Ablesekarte.
    pub ablesekarte: crate::utilmd_stammdaten::Kunde,
}

/// The body of [`GELI_LIEFERENDE_ANMELDEN`] — the Abmeldung NN 44004, LF → NB
/// (UTILMD AHB Gas 1.2 Kap. 5.5, S. 38–43).
///
/// The column makes `SG4 STS+7` Muss (`[2061]`), so the Transaktionsgrund is
/// required; the door holds the rendered 44004 against its profile, which says
/// which codes it admits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliLieferendeAnmelden {
    /// The Marktlokation — `SG5 LOC+172`.
    pub malo_id: String,
    /// `YYYYMMDD` (gesetzliche deutsche Zeit): the Lieferende `SG4 DTM+93`, or
    /// — under `ZG9`/`ZH1`/`ZH2`, the Aufhebung einer zukünftigen Zuordnung —
    /// the Beginn the NB confirmed, `SG4 DTM+92` (Bedingungen `[7]`, `[577]`).
    pub process_date: String,
    /// `SG4 STS+7` DE 9013 — one of the codes the 44004 column admits.
    pub transaktionsgrund: String,
}

/// The body of [`GELI_KUENDIGUNG_ANMELDEN`] — the Kündigung 44016, LFN → LFA
/// (UTILMD AHB Gas 1.2 Kap. 5.3).
///
/// The Kündigung goes to the Altlieferant, not the grid operator (`GeLi` Gas 3.0
/// Kap. 3.1), so the caller names him: nothing makod holds about the
/// Marktlokation says who supplies it today.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliKuendigungAnmelden {
    /// The Marktlokation — `SG5 LOC+172`.
    pub malo_id: String,
    /// The Kündigungstermin `SG4 DTM+93`, `YYYYMMDD`.
    pub process_date: String,
    /// The Altlieferant's MP-ID — `NAD+MR` and the UNB recipient.
    pub lfa_mp_id: String,
}

/// The body of [`GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN`] — the Berechtigter's
/// Antwort auf Änderung.
///
/// The process is found by the Marktlokation; the Änderung it answers fixes
/// the Antwort PID and the Codeliste the code must come from — `G_0016` for
/// the 44115, which publishes `E15` alone (EBD 4.3 Kap. 13.10).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliStammdatenAenderungBeantworten {
    /// The Marktlokation the Änderung named — `SG5 LOC+172`.
    pub malo_id: String,
    /// `SG4 STS+E01` DE 9013 — a code of the Antwort column's Codeliste.
    pub antwort_code: String,
}

/// The body of [`GELI_STAMMDATEN_AENDERUNG_SENDEN`] — the Verantwortlicher's
/// Stammdatenänderung (UTILMD AHB Gas 1.2 Kap. 5.10–5.11).
///
/// On a nicht bilanzierungsrelevante 44112 / 44109 every data group of the
/// column is Soll `[92]` „Wenn Wert innerhalb SG bzw. Segment geändert wird"
/// (Kap. 5.10.1, S. 97–110; Kap. 5.11.3, S. 120–123), so `gruppen` names what
/// changed. The bilanzierungsrelevante 44123 and 44120 make `SG8` „Daten der
/// Marktlokation" Muss whole (Kap. 5.11.1, S. 114–116; Kap. 5.11.4, S. 127),
/// and the 44175 lists every Messlokation of the new structure (Kap. 5.11.2,
/// S. 119, `[590]`). The NB's read marktd on the Änderungsdatum; the LF's
/// carry its own facts, stated here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliStammdatenAenderungSenden {
    /// The Marktlokation — `SG5 LOC+172`.
    pub malo_id: String,
    /// `44109` / `44120` (LF → NB) or `44112` / `44123` / `44175` (NB → LF).
    pub pid: u32,
    /// The Berechtigter's MP-ID — `NAD+MR` and the UNB recipient.
    pub empfaenger_mp_id: String,
    /// Änderungsdatum `SG4 DTM+157`, `YYYYMMDD`.
    pub aenderungsdatum: String,
    /// Datum Vertragsbeginn `SG4 DTM+92`, `YYYYMMDD`.
    pub vertragsbeginn: String,
    /// The changed groups, by name: on a 44112 `TERMIN_NETZNUTZUNGSABRECHNUNG`,
    /// `NETZNUTZUNGSABRECHNUNGSINTERVALL`, `MARKTGEBIET`,
    /// `DRUCKEBENE`, `NETZNUTZUNG`, `OBIS_DATEN`, `KONZESSIONSABGABE`,
    /// `GEMEINDERABATT`, `ZUGEORDNETE_MARKTPARTNER`, `GASQUALITAET`,
    /// `MARKTLOKATIONSANSCHRIFT`; on a 44109 `ABRECHNUNGSINTERVALL_LF`,
    /// `GRUPPENZUORDNUNG`, `KUNDE_DES_LF`, `KORRESPONDENZANSCHRIFT_KUNDE_LF`.
    /// Empty on a 44120, 44123 and 44175, which carry their data whole.
    #[serde(default)]
    pub gruppen: Vec<String>,
    /// 44109: `SG6 DTM+Z20` Abrechnungsintervall des LF — 1, 3, 6 or 12.
    #[serde(default)]
    pub abrechnungsintervall_monate: Option<u8>,
    /// 44109: `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung nach `EnWG`.
    #[serde(default)]
    pub haushaltskunde: Option<bool>,
    /// 44109: `SG12 NAD+Z09` Kunde des LF, and with its `anschrift` the
    /// `NAD+Z04` Korrespondenzanschrift.
    #[serde(default)]
    pub kunde: Option<crate::utilmd_stammdaten::Kunde>,
    /// 44120: `SG8 SEQ+Z01` `SG10 CCI+Z19` Bilanzkreis, Muss.
    #[serde(default)]
    pub bilanzkreis: Option<String>,
}

/// The body of `gpke.neuanlage.anmelden` — the Lieferant's Anmeldung neue
/// Marktlokation, LF → NB: **55600** verbrauchend (UTILMD AHB Strom 2.2 Kap.
/// 8.4, S. 95–108) or **55601** erzeugend (Kap. 8.5, S. 109–124).
///
/// A Marktlokation not yet in the grid has no record the door could resolve
/// its Netzbetreiber from, so the body names him; the MaLo-ID the Anmeldung
/// carries only „Wenn bekannt" (Bedingung `[165]`).
///
/// | Field | Segment | AHB S. |
/// |---|---|---|
/// | `pid` | `SG6 RFF+Z13` — 55600 or 55601 | 99, 113 |
/// | `nb_mp_id` | `SG2 NAD+MR` | 95, 109 |
/// | `malo_id` | `SG5 LOC+Z16`, Soll `[165]` | 97, 112 |
/// | `vertragsbeginn` | `SG4 DTM+92`, `YYYY-MM-DD` | 96, 110 |
/// | `transaktionsgrund_ergaenzung` | `SG4 STS+7` beside `E02`: 55600 `ZW4` (the default), 55601 `ZW0` / `ZW2` | 96, 110 |
/// | `bilanzkreis` | `SG8 SEQ+Z79` Produktpaket (Kap. 5.3) | 100, 114 |
/// | `tranchengroesse` | `SG8 SEQ+Z79` Produkt `9991000002090` — Muss beside `ZW2` (Codeliste der Konfigurationen 1.4 Kap. 6.1.1) | — |
/// | `haushaltskunde` | `SG8 SEQ+Z01` `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung — 55600 only | 104 |
/// | `kunde` | `SG12 NAD+Z09` / `NAD+Z04` — Muss on 55600, Kann on 55601 | 106–107, 122 |
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeNeuanlageAnmelden {
    /// `55600` verbrauchende or `55601` erzeugende Marktlokation.
    pub pid: u32,
    /// The Netzbetreiber — `NAD+MR` and the UNB recipient.
    pub nb_mp_id: String,
    /// The Marktlokation, where already known — `SG5 LOC+Z16`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<String>,
    /// `SG4 DTM+92` Datum Vertragsbeginn, `YYYY-MM-DD`.
    pub vertragsbeginn: String,
    /// `SG4 STS+7` Ergänzung — the Geschäftsvorfall on a 55601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG8 SEQ+Z79` / `SG10 CAV+ZV4` — the Bilanzkreis, an X- or Y-EIC.
    pub bilanzkreis: String,
    /// The Tranchengröße of Geschäftsvorfall 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tranchengroesse: Option<Tranchengroesse>,
    /// `true` for `CCI+++Z15`, a Haushaltskunde; `false` for `Z18` — 55600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haushaltskunde: Option<bool>,
    /// The Kunde des Lieferanten with his Korrespondenzanschrift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kunde: Option<crate::utilmd_stammdaten::Kunde>,
}

/// A Tranchengröße as the Codeliste der Konfigurationen 1.4 Kap. 6.1.1 names
/// it: the Produkteigenschaft (`prozent`, `aufteilungsfaktor`,
/// `technische_ressourcen`) and its value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tranchengroesse {
    /// The Produkteigenschaft.
    pub art: String,
    /// Its value — on `prozent` the share in per cent.
    pub wert: String,
}

/// The body of [`GPKE_NEUANLAGE_BESTAETIGEN`] and [`GPKE_NEUANLAGE_ABLEHNEN`] —
/// the NB's answer to an Anmeldung neue Marktlokation 55600 / 55601 (UTILMD
/// AHB Strom 2.2 Kap. 8.4, S. 96–109; Kap. 8.5, S. 110–124). See
/// [`GpkeEogAnmelden`] for why it is shared.
///
/// The Bestätigungen 55602 / 55603 tell the LF what the NB knows of the new
/// Marktlokation, and only the NB knows it:
///
/// | Field | Segment | AHB |
/// |---|---|---|
/// | `transaktionsgrund_ergaenzung` | `SG4 STS+7` DE 9013: 55602 `ZW6` pauschal / `ZW7` gemessen; 55603 the Anmeldung's `ZW0` / `ZW2` | S. 97, 111 |
/// | `geplantes_produktpaket` | `SG6 RFF+Z60`, the Anmeldung's `SEQ+Z79` Produktpaket-ID by default | S. 100, 115 |
/// | `messlokationen` | `SG5 LOC+Z17` and one `SG8 SEQ+ZF3` each — on 55602 behind `ZW7` (`[483]`), on 55603 always (`[623]`) | S. 100, 106, 114, 122 |
/// | `malo_msb` | `SG8 SEQ+Z98` `SG10 CCI+++ZB3` — wherever Messlokationen are named | S. 105, 119 |
/// | `tranche` | `SG5 LOC+Z21` and `SG8 SEQ+Z15` — on a 55603 behind `ZW2` (`[478]`), nowhere else | S. 113, 119–120 |
///
/// An Ablehnung carries none of them. Either answer names the Marktlokation by
/// `malo_id`, or — when the NB could not identify it — the process by
/// `process_id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeNeuanlageBeantworten {
    /// The Marktlokation — `SG5 LOC+Z16`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<String>,
    /// The Neuanlage process, for a Marktlokation the NB could not identify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    /// `SG4 STS+E01` DE 9013 — the `E_0608` Antwortcode.
    pub antwort_code: String,
    /// `SG4 STS+E01` DE 1131 — `E_0608` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_codeliste: Option<String>,
    /// `SG4 FTX+ACB` — Muss beside `A99`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bemerkung: Option<String>,
    /// `SG4 STS+7` DE 9013 Ergänzung of a Bestätigung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG6 RFF+Z60` — the Produktpaket-ID the NB will implement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geplantes_produktpaket: Option<String>,
    /// `SG8 SEQ+Z98` — the Messstellenbetreiber of the Marktlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_msb: Option<AntwortMsb>,
    /// `SG5 LOC+Z17` / `SG8 SEQ+ZF3` — the Messlokationen, each with its MSB.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messlokationen: Vec<AntwortMesslokation>,
    /// `SG5 LOC+Z21` / `SG8 SEQ+Z15` — the Tranche of Geschäftsvorfall 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tranche: Option<AntwortTranche>,
}

/// The Tranche a Bestätigung 55603 names in Geschäftsvorfall 3 (UTILMD AHB
/// Strom 2.2 Kap. 8.5, S. 113, 119–120).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AntwortTranche {
    /// `SG5 LOC+Z21` — the Tranche's ID, in the format of a Marktlokations-ID.
    pub tranche_id: String,
    /// `SG10 CCI+Z37` — `ZD1` prozentual, `ZD2` Aufteilungsfaktor, `ZH1`
    /// Aufteilung auf Technische Ressourcen.
    pub basis: String,
    /// `SG9 QTY+11` — the share in per cent, beside `ZD1` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prozent: Option<String>,
}

/// A Messstellenbetreiber as a Bestätigung Anmeldung names it: `SG10
/// CCI+++ZB3`, `CAV+Z91:<mp_id>::<rolle>:<grundlage>` and `CAV+ZF0:<gmsb_mp_id>`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AntwortMsb {
    /// `CAV+Z91` DE 1131 — the MSB that operates the Messstelle.
    pub mp_id: String,
    /// `CAV+Z91` DE 7110 — `Z39` grundzuständig, `Z40` wettbewerblich, `Z41`
    /// Auffang.
    pub rolle: String,
    /// `CAV+Z91` DE 7110, second occurrence — `Z19` vertragliche Grundlage,
    /// `Z20` Weiterverpflichtung durch den gMSB.
    pub grundlage: String,
    /// `CAV+ZF0` DE 1131 — the grundzuständiger MSB of the Lokation.
    pub gmsb_mp_id: String,
}

/// One Messlokation a Bestätigung names, with its Messstellenbetreiber.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AntwortMesslokation {
    /// `SG5 LOC+Z17` / `SG8 RFF+Z19` — the Messlokations-ID, 33 characters.
    pub melo_id: String,
    /// The Messstellenbetreiber of this Messlokation.
    pub msb: AntwortMsb,
}

impl GeliLieferbeginnAnmelden {
    /// Every value the 44001 column cannot carry, each naming its segment —
    /// empty when the body is sendable.
    ///
    /// The Transaktionsgrund is not checked here: the codes the column admits
    /// are the profile's to say, and the door that holds the profile checks
    /// them.
    #[must_use]
    pub fn maengel(&self) -> Vec<String> {
        let mut m = Vec::new();
        for (field, value) in [
            ("malo_id (SG5 LOC+172)", &self.malo_id),
            ("process_date (SG4 DTM+92)", &self.process_date),
            ("transaktionsgrund (SG4 STS+7)", &self.transaktionsgrund),
            ("bilanzkreis (SG10 CCI+Z19)", &self.bilanzkreis),
        ] {
            if value.trim().is_empty() {
                m.push(format!("{field} is empty"));
            }
        }
        if !ABRECHNUNGSINTERVALLE_MONATE.contains(&self.abrechnungsintervall_monate) {
            m.push(format!(
                "abrechnungsintervall_monate {} (SG6 DTM+Z20): UTILMD MIG Gas G1.2 admits {:?}",
                self.abrechnungsintervall_monate, ABRECHNUNGSINTERVALLE_MONATE
            ));
        }
        if self.netznutzung_zahler == crate::utilmd_stammdaten::NetznutzungZahler::Kunde
            && self.netznutzungsvertrag != crate::utilmd_stammdaten::Netznutzungsvertrag::KundeNb
        {
            m.push(
                "netznutzung_zahler KUNDE (CAV+Z73:::Z10) only beside a direkter Vertrag \
                 zwischen Kunden und NB (CAV+Z74:::Z08, Bedingung [216])"
                    .to_owned(),
            );
        }
        let rabatt = self.gemeinderabatt_prozent;
        if rabatt < rust_decimal::Decimal::ZERO
            || rabatt > rust_decimal::Decimal::TEN
            || rabatt.normalize().scale() > 2
        {
            m.push(format!(
                "gemeinderabatt_prozent {rabatt} (SG9 QTY+Z16): ≥ 0, ≤ 10, at most two \
                 Nachkommastellen (Bedingungen [902], [938], [930])"
            ));
        }
        for (rolle, partei) in [
            ("kunde (SG12 NAD+Z09/Z04)", &self.kunde),
            ("ablesekarte (SG12 NAD+Z05)", &self.ablesekarte),
        ] {
            m.extend(
                partei
                    .anmeldung_maengel()
                    .into_iter()
                    .map(|f| format!("{rolle}: {f}")),
            );
        }
        m
    }
}

/// The `payload` of [`GELI_EOG_BESTAETIGEN`] carrying a Zustimmung — the E/G
/// Lieferant's Bestätigung `EoG` 44014. See [`GpkeEogAnmelden`] for why it is
/// shared.
///
/// `makod` reads the Marktlokation's record from `marktd`; the body carries
/// the Antwortcode and the two facts of the LF's own that UTILMD AHB Gas 1.2
/// Kap. 5.9 makes Muss on a 44014 and that no record holds. Both are `Option`
/// so the door can refuse a missing one by name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliEogBestaetigen {
    /// The Marktlokation; `makod` finds the process by it.
    pub malo_id: String,
    /// The `E_3008` Antwortcode (`SG4 STS+E01`); its Cluster picks 44014.
    pub antwort_code: String,
    /// `SG10 CCI+Z19` — the LF's Bilanzkreis (AHB S. 80).
    #[serde(default)]
    pub bilanzkreis: Option<String>,
    /// `SG6 DTM+Z20` „Abrechnungsintervall des LF", DE 2379 `802` Monat (AHB
    /// S. 78), one of [`ABRECHNUNGSINTERVALLE_MONATE`].
    #[serde(default)]
    pub abrechnungsintervall_monate: Option<u8>,
    /// `SG4 FTX+ACB` Erläuterung.
    #[serde(default)]
    pub bemerkung: Option<String>,
    /// The Erläuterung under the key an operator's decision writes it to;
    /// read where `bemerkung` is absent.
    #[serde(default)]
    pub reason: Option<String>,
    /// The process the answering service decided on. Echoed for the audit
    /// trail; `makod` addresses the process by `malo_id`.
    #[serde(default)]
    pub process_id: Option<String>,
    /// The Cluster as the answering service read it. Echoed; `makod` reads
    /// the Cluster from `antwort_code` itself.
    #[serde(default)]
    pub zustimmung: Option<bool>,
    /// The Codeliste the code was drawn from (`E_3008`). Echoed; `makod`
    /// resolves the code in `E_3008` itself.
    #[serde(default)]
    pub antwort_codeliste: Option<String>,
    /// The Entscheidungsbaum an operator named. Echoed, as `antwort_codeliste`.
    #[serde(default)]
    pub antwort_tree: Option<String>,
    /// The Termin the walk checked, `YYYYMMDD`. Echoed; the 44014 states the
    /// process's own Zuordnungsbeginn.
    #[serde(default)]
    pub termin: Option<String>,
    /// Who released a held answer from processd's queue.
    #[serde(default)]
    pub approved_by: Option<String>,
    /// Who overrode a held Ablehnung from processd's queue, sending this
    /// Zustimmung instead.
    #[serde(default)]
    pub rejected_by: Option<String>,
}

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
