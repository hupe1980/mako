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

/// The supplier's answer, as the ERP or `processd` decides it.
///
/// Produced from an `mako-pruefung` walk: `antwort_code` and `ebd` are the
/// resolved Antwortcode, and `zustimmung` is that code's published **Cluster** —
/// not an independent judgement. A separate boolean could disagree with the
/// code, and send `A35` „Es besteht eine Vertragsbindung" as a Bestätigung.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub bemerkung: Option<String>,
    /// `SG4 DTM+93` — the date the answer states, `YYYYMMDD`.
    ///
    /// Several codes require the supplier's *own* date rather than the
    /// requested one: `A34` („teilt sein Lieferendedatum in der Antwort mit"),
    /// `A31`, and the Gas `Z01` „Zustimmung mit Terminänderung". `None` echoes
    /// the requested date.
    pub termin: Option<String>,
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
            termin: None,
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
            termin: None,
        }
    }

    /// Attach the `FTX+ACB` Erläuterung.
    #[must_use]
    pub fn with_bemerkung(mut self, text: impl Into<String>) -> Self {
        self.bemerkung = Some(text.into());
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
/// workflow; everything else is echoed from the request so the counterparty can
/// correlate it.
#[must_use]
pub fn antwort_outbox(
    response_pid: u32,
    antwort: &LfAntwort,
    location_id: &MaLo,
    sender: &MarktpartnerCode,
    receiver: &MarktpartnerCode,
    process_date: &str,
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
        payload["antwort_ebd"] = serde_json::Value::String(ebd.clone());
    }
    if let Some(bemerkung) = &antwort.bemerkung {
        payload["bemerkung"] = serde_json::Value::String(bemerkung.clone());
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
        )
        .payload
        .clone()
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
        assert_eq!(p["antwort_ebd"], "E_0624");
    }

    /// A Gas answer carries no DE 1131 — the MIG does not name its Codeliste.
    #[test]
    fn a_gas_answer_omits_the_ebd_reference() {
        let antwort = LfAntwort {
            antwort_code: "E15".to_owned(),
            ebd: None,
            zustimmung: true,
            bemerkung: None,
            termin: None,
        };
        assert!(outbox_for(&antwort).get("antwort_ebd").is_none());
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
