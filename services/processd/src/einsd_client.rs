//! The one thing `processd` asks the EEG-/KWKG-Register.
//!
//! Two Entscheidungsbäume need a fact the UTILMD cannot carry, and both facts
//! live in the same register row, so one call answers both:
//!
//! - `E_0622` Prüfschritte 400–830 choose an Anmeldung erzeugender
//!   Marktlokation's Vorlauffrist from the pair (bestehende, angemeldete)
//!   Veräußerungsform. The *angemeldete* one is on the wire (`SG10 CCI+Z22`);
//!   the **bestehende** one is register data.
//! - `E_0623` Prüfschritt 540 asks „Handelt es sich um eine
//!   direktvermarktungspflichtige Marktlokation?" — the § 21 Abs. 1 Satz 1 Nr. 1
//!   capacity question, decided over the Anlagen the register holds.
//!
//! Kept to a single read so the dependency stays legible: an NB deployment that
//! does not run `einsd` simply escalates every 55077, which is the § 20
//! EnWG-safe outcome and exactly what the engine does with a missing fact.

use mako_pruefung::nb::types::Veraeusserungsform;
use mako_service::http::{Upstream, UpstreamError};
use secrecy::SecretString;

/// What the register knows about a Marktlokation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterAuskunft {
    /// The Veräußerungsform in force, as `SG10 CCI+Z22` DE 7037 spells it.
    pub veraeusserungsform: Veraeusserungsform,
    /// `true` for the Ausfallvergütung (§ 21 Abs. 1 Satz 1 **Nr. 3** EEG 2023 —
    /// Nr. 2 is the unentgeltliche Abnahme; under the EEG 2017 it was Nr. 2).
    ///
    /// Wire code `Z90` covers it *and* die uneingeschränkte Einspeisevergütung,
    /// and the two take different Vorlauffristen — a month versus die verkürzte
    /// fünf Werktage — so this flag is one of the two reasons for the lookup.
    pub ausfallverguetung: bool,
    /// `E_0623` Prüfschritt 540 — „Handelt es sich um eine
    /// direktvermarktungspflichtige Marktlokation?"
    ///
    /// `None` when the register cannot answer it, which happens for a plant
    /// commissioned before 2016: the EEG 2014 staging that governs it is not in
    /// mako's regulatory corpus, so `einsd` reports the question as open rather
    /// than answering „nein". `processd` then builds no `TranchenLage` and
    /// `E_0623` escalates — the same treatment every unread fact gets.
    pub direktvermarktungspflichtig: Option<bool>,
}

/// `einsd`'s answer, as the handler renders it.
#[derive(serde::Deserialize)]
struct VeraeusserungsformBody {
    /// The `CCI+Z22` DE 7037 code, absent for a settlement model that has none.
    veraeusserungsform: Option<String>,
    #[serde(default)]
    ausfallverguetung: bool,
    /// Absent on an older `einsd`, which is indistinguishable from „the register
    /// cannot answer it" — both leave Prüfschritt 540 open.
    #[serde(default)]
    direktvermarktungspflichtig: Option<bool>,
}

/// Reader for `einsd`'s Veräußerungsform lookup.
#[derive(Debug, Clone)]
pub struct EinsdClient(Upstream);

impl EinsdClient {
    /// Address `einsd` at `base_url`, sharing the daemon's HTTP client.
    #[must_use]
    pub fn new(base_url: &str, api_key: Option<SecretString>, client: reqwest::Client) -> Self {
        Self(Upstream::new("einsd", base_url, api_key, client))
    }

    /// The Veräußerungsform in force at `malo_id`.
    ///
    /// `Ok(None)` means the register has no plant with that MaLo-ID, or its
    /// settlement model is not a Veräußerungsform at all (Mieterstrom, GGV,
    /// Eigenverbrauch, Post-EEG). Neither is evidence of a
    /// „Nicht-EEG-/-KWKG"-Marktlokation, so the caller escalates rather than
    /// choosing a Frist.
    ///
    /// # Errors
    ///
    /// Transport failures. A decision must not be made on a missing answer that
    /// only failed to arrive, so these propagate and the event is redelivered.
    pub async fn veraeusserungsform(
        &self,
        malo_id: &str,
    ) -> Result<Option<RegisterAuskunft>, UpstreamError> {
        let path = format!("/api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform");
        let Some(body) = self
            .0
            .json::<VeraeusserungsformBody>(self.0.get(&path))
            .await?
        else {
            return Ok(None);
        };
        let Some(form) = body
            .veraeusserungsform
            .as_deref()
            .and_then(Veraeusserungsform::from_wire_code)
        else {
            return Ok(None);
        };
        Ok(Some(RegisterAuskunft {
            veraeusserungsform: form,
            ausfallverguetung: body.ausfallverguetung,
            direktvermarktungspflichtig: body.direktvermarktungspflichtig,
        }))
    }
}
