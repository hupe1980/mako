//! The answer an LF sends on an NB- or LFN-initiated process.
//!
//! Three GPKE processes put the supplier in the answering seat — the
//! Ankündigung der Beendigung der Zuordnung (55007), the Anfrage zur Beendigung
//! der Zuordnung (55010) and the Kündigung (55016) — plus the Ankündigung
//! Zuordnung LF (55607) the NB sends the incoming supplier. All four answer the
//! same way, so they share one command payload and one outbox builder.
//!
//! # Two things every answer needs
//!
//! 1. **A message.** `SendAntwort` emits a [`PendingOutbox`]. An
//!    `AntwortGesendet` event without one records the process as answered while
//!    the counterparty watches its Frist expire.
//! 2. **An Antwortcode.** The AHB marks `SG4 STS+E01` **Muss** on every
//!    Antwortnachricht and restricts the code to the answering EBD's cluster.
//!    Free text in its place is not an answer.
//!
//! [`PendingOutbox`]: mako_engine::outbox::PendingOutbox

use mako_engine::outbox::PendingOutbox;
use mako_engine::types::{MaLo, MarktpartnerCode};

/// The `SG4` facts an LF-answered Vorgang carries, and the
/// `de.mako.process.initiated` it builds.
///
/// Defined in [`mako_engine::lf_vorgang`] rather than here because GeLi Gas
/// emits the same contract for 44007 / 44010 / 44016 and `processd` parses both
/// with one parser. Two definitions drifted once already.
pub use mako_engine::lf_vorgang::LfVorgangsdaten;

/// The supplier's answer, as the ERP or `processd` decides it.
///
/// Produced from an `mako-pruefung` walk: `antwort_code` and `ebd` are the
/// resolved Antwortcode, and `zustimmung` is that code's published **Cluster** —
/// not an independent judgement. A separate boolean could disagree with the
/// code, and send `A35` „Es besteht eine Vertragsbindung" as a Bestätigung.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LfAntwort {
    /// `SG4 STS+E01` DE 9013 — the EBD Antwortcode (`A10`, `A35`, `E15`, …).
    pub antwort_code: String,
    /// `SG4 STS+E01` DE 1131 — the EBD it comes from (`E_0609`, `E_0624`, …).
    ///
    /// `None` on the Gas Codelisten, which the MIG does not name in DE 1131.
    pub ebd: Option<String>,
    /// `true` when the code sits in the Zustimmungs-Cluster — this is what
    /// selects the Bestätigungs- over the Ablehnungs-PID.
    pub zustimmung: bool,
    /// `FTX+ACB` Erläuterung, mandatory alongside the catch-all codes
    /// (`A99` Strom, `E14` Gas) and wherever the EBD says „ist in der Antwort
    /// zu beschreiben".
    ///
    /// A remark and nothing else. On a 55608 the UTILMD AHB admits the segment
    /// under Bedingung `[48]` — „Wenn in dieser SG4 das STS+E01++A99 vorhanden" —
    /// which is the 55609 Ablehnung, so a Bilanzkreis carried here rides a
    /// segment the Bestätigung may not contain. It has its own field.
    pub bemerkung: Option<String>,
    /// `SG8 SEQ+Z79` / `SG10 CAV+ZV4` — the Bilanzkreis a Zuordnungs-Zustimmung
    /// names.
    ///
    /// `E_0603`–`E_0606` publish one Prüfschritt each, so the substance of a
    /// 55608 is not the code: it is which balancing circle the generation is
    /// booked into (GPKE Teil 2 § 2.4.2.2 Nr. 2). The Codeliste der
    /// Konfigurationen 1.4 Kap. 6.1.1 makes the Bilanzkreis product
    /// (`9991000002082`) unconditional in the Produktpaket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzkreis: Option<String>,
    /// `SG4 DTM+93` — the date the answer states, `YYYYMMDD`.
    ///
    /// Several codes require the supplier's *own* date rather than the
    /// requested one: `A34` („teilt sein Lieferendedatum in der Antwort mit"),
    /// `A31`, and the Gas `Z01` „Zustimmung mit Terminänderung". `None` echoes
    /// the requested date.
    pub termin: Option<String>,
    /// `SG4 STS+7` DE 9013 element 3 — the Transaktionsgrundergänzung of the
    /// **answer**, which is not the Anfrage's.
    ///
    /// A 55001 Anmeldung says `ZW4` „verbrauchende Marktlokation"; the 55002
    /// Bestätigung's Prüfschablone admits only `ZW6` „Pauschale
    /// Marktlokation", `ZW7` „Gemessene Marktlokation" and `ZAP` „Ruhende
    /// Marktlokation". The NB is the party that knows which of the three the
    /// Marktlokation is, so the classification is part of its answer rather
    /// than an echo of the request. Echoing `ZW4` is refused at the LFN with
    /// `AHB-55002-00036-STS-9013-CODE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_art: Option<String>,
    /// `SG6 RFF+Z60` DE 1154 — „Informativ zur Umsetzung geplantes
    /// Produktpaket", Muss on a Bestätigung Anmeldung.
    ///
    /// The `SG8 SEQ+Z79` DE 1050 Produktpaket-ID from the Anmeldung that the
    /// NB will actually implement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geplantes_produktpaket: Option<String>,
    /// `SG8 SEQ+Z98` `SG10 CCI+++ZB3` / `CAV+Z91` — the Messstellenbetreiber
    /// the NB has assigned to the Marktlokation.
    ///
    /// Muss inside the „Daten der Marktlokation" block a `ZW7` answer opens:
    /// the Bestätigung is where the LFN learns who meters the point it is about
    /// to supply, and it cannot look it up anywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_msb: Option<ZugeordneterMsb>,
    /// `SG5 LOC+Z17` — the Messlokationen behind the Marktlokation.
    ///
    /// Muss whenever [`malo_art`](Self::malo_art) is `ZW7` „Gemessene
    /// Marktlokation" (UTILMD AHB Strom Bedingung `[483]`), which is also what
    /// opens the `SG8 SEQ+Z98` „Daten der Marktlokation" block. Bedingung
    /// `[623]`: „Es sind alle Identifikatoren der Messlokationen anzugeben, die
    /// zur Ermittlung der Energiemenge der im Vorgang genannten Marktlokation
    /// benötigt werden" — so a MaLo fed by two Messlokationen names both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messlokationen: Vec<AntwortMesslokation>,
}

/// What a GPKE Antwort-PID's Prüfschablone admits, beyond the Antwortcode.
///
/// The Bestätigung and the Ablehnung of the same Anfrage are **not** the same
/// message with one code changed. UTILMD AHB Strom 2.1:
///
/// | | 55002 | 55003 | 55005 | 55006 | 55078 | 55080 |
/// |---|---|---|---|---|---|---|
/// | `BGM` DE 1001 | `E01` | `E01` | `E02` | `E02` | `E01` | `E01` |
/// | `STS+7` Ergänzung | `ZW6`/`ZW7`/`ZAP` | `ZW4` | — | — | `ZW0`/`ZW1`/`ZW2` | `ZW3` |
/// | `SG4 DTM` | `92` | — | `93` | — | `92` | — |
/// | `SG5 LOC`, `SG8` Lokationsdaten | ✓ | — | — | — | ✓ | — |
/// | `SG6 RFF+Z60` | ✓ | — | — | — | ✓ | — |
///
/// An Ablehnung states no Lokation and no date: it says the Anfrage failed, and
/// everything the NB would have told the LFN about the Marktlokation is
/// something it is not going to supply. Sending the Bestätigung's blocks on an
/// Ablehnung is refused with eleven `NOT-PERMITTED` findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntwortForm {
    /// `BGM` DE 1001 — `E01` „Anmeldungen" or `E02` „Abmeldungen".
    ///
    /// Follows the Anfrage: the answer to an Abmeldung is itself an Abmeldung
    /// document. The renderer defaults to `E01`, so leaving it out sent both
    /// Abmeldungs-Antworten under the wrong Dokumentenname.
    pub document_code: &'static str,
    /// `SG4 STS+7` DE 9013 element 3, or `None` where the column lists none.
    ///
    /// `Some(None)` is not a case: a column either admits an Ergänzung or does
    /// not. Where it does and the value is the NB's own classification (55002),
    /// this is `None` and [`LfAntwort::malo_art`] supplies it.
    pub ergaenzung: Option<&'static str>,
    /// Whether the answer states the NB's classification of the Marktlokation
    /// itself — 55002 only, where the column admits three codes.
    pub klassifiziert_malo: bool,
    /// Whether `SG5 LOC`, the `SG8` Lokations-Datenblöcke and `SG6 RFF+Z60` are
    /// part of the Prüfschablone.
    pub lokationsdaten: bool,
}

impl AntwortForm {
    /// The form of one GPKE Antwort-PID, or `None` for a PID outside the family.
    #[must_use]
    pub const fn of(pid: u32) -> Option<Self> {
        let (document_code, ergaenzung, klassifiziert_malo, lokationsdaten) = match pid {
            // Bestätigung Anmeldung verb. MaLo — the NB says whether the
            // Marktlokation is pauschal, gemessen or ruhend.
            55002 => ("E01", None, true, true),
            // Ablehnung Anmeldung verb. MaLo — `ZW4` and nothing else.
            55003 => ("E01", Some("ZW4"), false, false),
            // Bestätigung / Ablehnung Abmeldung — `E02`, and no Ergänzung.
            55005 | 55006 => ("E02", None, false, false),
            // Bestätigung Anmeldung erz. MaLo — echoes the 55077's `ZW0`/`ZW1`/
            // `ZW2`, which says what kind of Erzeugung it is.
            55078 => ("E01", None, false, true),
            // Ablehnung Anmeldung erz. MaLo.
            55080 => ("E01", Some("ZW3"), false, false),
            _ => return None,
        };
        Some(Self {
            document_code,
            ergaenzung,
            klassifiziert_malo,
            lokationsdaten,
        })
    }
}

/// The Messstellenbetreiber of one Lokation, as a Bestätigung names it.
///
/// `SG10 CCI+++ZB3` „Zugeordneter Marktpartner", then
/// `CAV+Z91:<mp_id>::<rolle>:<grundlage>` and `CAV+ZF0:<gmsb_mp_id>`. All three
/// codes are Muss on a Bestätigung Anmeldung: the acting MSB, on what basis it
/// acts, and which grundzuständiger MSB stands behind the Messstelle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZugeordneterMsb {
    /// `CAV+Z91` DE 1131 — the MSB that operates the Messstelle.
    pub mp_id: String,
    /// `CAV+Z91` DE 7110, first occurrence — `Z39` grundzuständig, `Z40`
    /// wettbewerblich, `Z41` Auffang.
    ///
    /// Not cosmetic: it decides who the LF settles the Messentgelt with.
    pub rolle: String,
    /// `CAV+Z91` DE 7110, second occurrence — `Z19` „auf vertraglicher
    /// Grundlage gegenüber Anschlussnutzer / Anschlussnehmer" or `Z20` „in der
    /// Ausübung der Weiterverpflichtung durch den gMSB".
    pub grundlage: String,
    /// `CAV+ZF0` DE 1131 — the grundzuständiger MSB of this Lokation.
    ///
    /// Equal to [`mp_id`](Self::mp_id) when `rolle` is `Z39`, and a different
    /// party otherwise; the LFN has no other way to learn who it is.
    pub gmsb_mp_id: String,
}

impl ZugeordneterMsb {
    /// The grundzuständige Messstellenbetreiber of a Lokation, acting on a
    /// contract with the Anschlussnutzer — `Z39` + `Z19`, the ordinary case.
    #[must_use]
    pub fn grundzustaendig(mp_id: impl Into<String>) -> Self {
        let mp_id = mp_id.into();
        Self {
            rolle: "Z39".to_owned(),
            grundlage: "Z19".to_owned(),
            gmsb_mp_id: mp_id.clone(),
            mp_id,
        }
    }

    /// A wettbewerblicher Messstellenbetreiber — `Z40` — with the gMSB it
    /// displaced, which the LFN has no other way to learn.
    #[must_use]
    pub fn wettbewerblich(mp_id: impl Into<String>, gmsb_mp_id: impl Into<String>) -> Self {
        Self {
            mp_id: mp_id.into(),
            rolle: "Z40".to_owned(),
            grundlage: "Z19".to_owned(),
            gmsb_mp_id: gmsb_mp_id.into(),
        }
    }
}

/// One Messlokation a `ZW7` answer names, with the MSB assigned to it.
///
/// Bedingung `[623]`: „Es sind alle Identifikatoren der Messlokationen
/// anzugeben, die zur Ermittlung der Energiemenge der im Vorgang genannten
/// Marktlokation benötigt werden" — a MaLo fed by two Messlokationen names
/// both, and they can have different Messstellenbetreiber.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AntwortMesslokation {
    /// `SG5 LOC+Z17` DE 3225 and `SG8 RFF+Z19` DE 1154 — 33 characters.
    pub melo_id: String,
    /// The Messstellenbetreiber of *this* Messlokation.
    pub msb: ZugeordneterMsb,
}

impl LfAntwort {
    /// A Zustimmung drawn from a named EBD.
    #[must_use]
    pub fn zustimmung(antwort_code: impl Into<String>, ebd: impl Into<String>) -> Self {
        Self {
            antwort_code: antwort_code.into(),
            ebd: Some(ebd.into()),
            zustimmung: true,
            bemerkung: None,
            bilanzkreis: None,
            termin: None,
            malo_art: None,
            geplantes_produktpaket: None,
            malo_msb: None,
            messlokationen: Vec::new(),
        }
    }

    /// An Ablehnung drawn from a named EBD.
    #[must_use]
    pub fn ablehnung(antwort_code: impl Into<String>, ebd: impl Into<String>) -> Self {
        Self {
            antwort_code: antwort_code.into(),
            ebd: Some(ebd.into()),
            zustimmung: false,
            bemerkung: None,
            bilanzkreis: None,
            termin: None,
            malo_art: None,
            geplantes_produktpaket: None,
            malo_msb: None,
            messlokationen: Vec::new(),
        }
    }

    /// State the NB's classification of the Marktlokation —
    /// [`malo_art`](Self::malo_art), `ZW6` / `ZW7` / `ZAP`.
    #[must_use]
    pub fn with_klassifizierung(mut self, malo_art: impl Into<String>) -> Self {
        self.malo_art = Some(malo_art.into());
        self
    }

    /// State which Produktpaket-ID the NB will implement —
    /// [`geplantes_produktpaket`](Self::geplantes_produktpaket), `SG6 RFF+Z60`.
    #[must_use]
    pub fn with_produktpaket(mut self, paket_id: impl Into<String>) -> Self {
        self.geplantes_produktpaket = Some(paket_id.into());
        self
    }

    /// Name one Messlokation behind the Marktlokation and its
    /// Messstellenbetreiber, and assign the same MSB to the Marktlokation.
    ///
    /// The ordinary case: one MaLo, one MeLo, one MSB. A MaLo fed by several
    /// Messlokationen calls this once per MeLo; one whose MaLo-MSB differs sets
    /// [`malo_msb`](Self::malo_msb) afterwards.
    #[must_use]
    pub fn with_messlokation(mut self, melo_id: impl Into<String>, msb: ZugeordneterMsb) -> Self {
        if self.malo_msb.is_none() {
            self.malo_msb = Some(msb.clone());
        }
        self.messlokationen.push(AntwortMesslokation {
            melo_id: melo_id.into(),
            msb,
        });
        self
    }

    /// Attach the `FTX+ACB` Erläuterung.
    #[must_use]
    pub fn with_bemerkung(mut self, text: impl Into<String>) -> Self {
        self.bemerkung = Some(text.into());
        self
    }

    /// Name the Bilanzkreis of a Zuordnungs-Zustimmung (`SG8 SEQ+Z79`).
    #[must_use]
    pub fn with_bilanzkreis(mut self, bk: impl Into<String>) -> Self {
        self.bilanzkreis = Some(bk.into());
        self
    }

    /// State a date other than the requested one (`YYYYMMDD`).
    #[must_use]
    pub fn with_termin(mut self, yyyymmdd: impl Into<String>) -> Self {
        self.termin = Some(yyyymmdd.into());
        self
    }
}

/// Build the outbound UTILMD that carries an LF answer.
///
/// `response_pid` is already resolved from [`LfAntwort::zustimmung`] by the
/// workflow.
///
/// `referenz_vorgangsnummer` is the request's `SG4 IDE+24` DE 7402, which the
/// answer carries back in `SG4 RFF+TN` — „Referenz Vorgangsnummer (aus
/// Anfragenachricht)", **Muss** on every UTILMD Antwortnachricht (UTILMD AHB
/// Strom 2.2 / Gas 1.2, SG6 RFF 1153 `TN`). It is *not* reused as the answer's
/// own `IDE+24`: the MIG requires DE 7402 to be unique across every `IDE+24`
/// and `IDE+Z01` ever sent, so an answer that echoed it would collide with the
/// request.
#[must_use]
pub fn antwort_outbox(
    response_pid: u32,
    antwort: &LfAntwort,
    location_id: &MaLo,
    sender: &MarktpartnerCode,
    receiver: &MarktpartnerCode,
    process_date: &str,
    referenz_vorgangsnummer: Option<&str>,
) -> PendingOutbox {
    let mut payload = serde_json::json!({
        "pid":          response_pid,
        // The answer travels back the way the request came: our own MP-ID as
        // sender, the requester as receiver.
        "sender":       receiver.as_str(),
        "receiver":     sender.as_str(),
        "malo":         location_id.as_str(),
        "process_date": antwort.termin.as_deref().unwrap_or(process_date),
        "antwort_code": antwort.antwort_code,
    });
    if let Some(ebd) = &antwort.ebd {
        payload["antwort_codeliste"] = serde_json::Value::String(ebd.clone());
    }
    if let Some(bemerkung) = &antwort.bemerkung {
        payload["bemerkung"] = serde_json::Value::String(bemerkung.clone());
    }
    if let Some(bilanzkreis) = &antwort.bilanzkreis {
        payload["bilanzkreis"] = serde_json::Value::String(bilanzkreis.clone());
    }
    if let Some(referenz) = referenz_vorgangsnummer {
        payload["referenz_vorgangsnummer"] = serde_json::Value::String(referenz.to_owned());
    }
    PendingOutbox::new("UTILMD", sender.as_str(), payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox_for(antwort: &LfAntwort) -> serde_json::Value {
        antwort_outbox(
            55_009,
            antwort,
            &MaLo::new("51238696012"),
            &MarktpartnerCode::new("9900357000004"),
            &MarktpartnerCode::new("9900000000001"),
            "20260901",
            Some("NNV1234"),
        )
        .payload
        .clone()
    }

    /// `SG4 RFF+TN` — the AHB marks it Muss on every Antwortnachricht, and it
    /// is the only thing that ties the answer to the request: `IDE+24` must be
    /// a *fresh* Vorgangsnummer, never the requester's.
    #[test]
    fn the_answer_back_references_the_requests_vorgangsnummer() {
        let p = outbox_for(&LfAntwort::zustimmung("A10", "E_0609"));
        assert_eq!(p["referenz_vorgangsnummer"], "NNV1234");
        assert_ne!(
            p.get("vorgangsnummer").and_then(serde_json::Value::as_str),
            Some("NNV1234"),
            "IDE+24 DE 7402 must stay globally unique (UTILMD MIG S2.2, Hinweis zu DE7402)"
        );
    }

    /// The parties are swapped: the NB sent the request, so the NB receives the
    /// answer.
    #[test]
    fn the_answer_travels_back_to_the_requester() {
        let p = outbox_for(&LfAntwort::ablehnung("A35", "E_0624"));
        assert_eq!(p["sender"], "9900000000001");
        assert_eq!(p["receiver"], "9900357000004");
    }

    /// The Antwortcode and its EBD both reach the renderer — DE 9013 and
    /// DE 1131 of `SG4 STS+E01`.
    #[test]
    fn the_antwortcode_and_its_ebd_are_both_carried() {
        let p = outbox_for(&LfAntwort::ablehnung("A35", "E_0624"));
        assert_eq!(p["antwort_code"], "A35");
        assert_eq!(p["antwort_codeliste"], "E_0624");
    }

    /// A Gas answer carries no DE 1131 — the MIG does not name its Codeliste.
    #[test]
    fn a_gas_answer_omits_the_ebd_reference() {
        let antwort = LfAntwort {
            antwort_code: "E15".to_owned(),
            ebd: None,
            zustimmung: true,
            bemerkung: None,
            bilanzkreis: None,
            termin: None,
            ..LfAntwort::ablehnung("", "")
        };
        assert!(outbox_for(&antwort).get("antwort_codeliste").is_none());
    }

    /// `A34` states the supplier's own Lieferendedatum, not the requested one.
    #[test]
    fn a_stated_termin_replaces_the_requested_date() {
        let p = outbox_for(&LfAntwort::zustimmung("A34", "E_0624").with_termin("20260831"));
        assert_eq!(p["process_date"], "20260831");
    }

    /// Without one, the requested date is echoed.
    #[test]
    fn an_answer_without_its_own_date_echoes_the_request() {
        let p = outbox_for(&LfAntwort::zustimmung("A10", "E_0609"));
        assert_eq!(p["process_date"], "20260901");
    }

    /// The Erläuterung reaches `FTX+ACB`.
    #[test]
    fn a_bemerkung_is_carried() {
        let p =
            outbox_for(&LfAntwort::ablehnung("A99", "E_0609").with_bemerkung("Zählerstand fehlt"));
        assert_eq!(p["bemerkung"], "Zählerstand fehlt");
    }
}
