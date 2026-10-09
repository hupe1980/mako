//! Typed outbound payloads — one struct per message kind, shared by the
//! workflow that produces the outbox entry and the renderer that reads it.
//!
//! An outbox entry's `payload` is stored as JSON, and a renderer that reads it
//! back by string key cannot notice that the producer spelled the key
//! differently: the field is simply absent, and a Muss segment drops out of the
//! message or the entry dead-letters. With a struct on both sides, a renamed or
//! retyped field is a compile error in whichever side did not follow.
//!
//! `serde_json::Value` remains only at the storage boundary
//! ([`PendingOutbox::typed`](crate::outbox::PendingOutbox::typed) serialises,
//! [`OutboundPayload::from_payload`] deserialises).
//!
//! The structs are **tolerant readers**, like event payloads: an outbox row is
//! written by one build and may be drained by the next, so a field is an
//! `Option` or carries `#[serde(default)]`, and no struct denies unknown fields.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// A payload type bound to the outbox `message_type` it travels under.
pub trait OutboundPayload: Serialize + DeserializeOwned {
    /// The outbox `message_type` — the EDIFACT message type it renders to.
    const MESSAGE_TYPE: &'static str;

    /// Read the payload of an outbox entry.
    ///
    /// # Errors
    ///
    /// The `serde_json` error when the stored JSON does not have this shape.
    fn from_payload(payload: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::deserialize(payload)
    }
}

// ── APERAK ────────────────────────────────────────────────────────────────────

/// APERAK Anwendungsfall **29001 Fehlermeldung** — `BGM+313`, `SG4`
/// Fehlerbeschreibung present.
pub const APERAK_PID_FEHLER: u32 = 29001;

/// APERAK Anwendungsfall **29002 Anerkennungsmeldung** — `BGM+312`, no `SG4`.
pub const APERAK_PID_ANERKENNUNG: u32 = 29002;

/// The payload of an `APERAK` outbox entry.
///
/// Built by [`AperakPayload::anerkennung`] or [`AperakPayload::fehler`], which
/// pair the Anwendungsfall with its `BGM` code: 29002 admits only `312`, 29001
/// only `313`. The renderer derives `BGM` from `error_code`, so a constructor
/// that set one without the other could not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AperakPayload {
    /// Sender MP-ID — the party acknowledging.
    pub sender: String,
    /// Receiver MP-ID — the sender of the acknowledged message. Absent means
    /// the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// The APERAK Anwendungsfall — [`APERAK_PID_ANERKENNUNG`] or
    /// [`APERAK_PID_FEHLER`].
    pub pid: u32,
    /// `UNH` DE 0062 of the acknowledged message: `SG2 RFF+ACE`, Muss in both
    /// Anwendungsfälle.
    pub orig_message_ref: String,
    /// `ERC` DE 9321 — present exactly on a Fehlermeldung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// `FTX+ABO` free text of a Fehlermeldung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `FTX+Z02` — where the AHB is violated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ortsangabe: Option<String>,
    /// The decision recorded for a Gas message that was processable: Gas has
    /// no Anerkennungsmeldung (APERAK AHB 1.1 §2.3), so the entry reaches the
    /// ERP and never the wire.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppress_wire: bool,
}

impl OutboundPayload for AperakPayload {
    const MESSAGE_TYPE: &'static str = "APERAK";
}

impl AperakPayload {
    /// An **Anerkennungsmeldung** (29002, `BGM+312`) from `sender` to
    /// `receiver` for the message `orig_message_ref`.
    #[must_use]
    pub fn anerkennung(sender: &str, receiver: &str, orig_message_ref: &str) -> Self {
        Self {
            sender: sender.to_owned(),
            receiver: Some(receiver.to_owned()),
            pid: APERAK_PID_ANERKENNUNG,
            orig_message_ref: orig_message_ref.to_owned(),
            error_code: None,
            reason: None,
            ortsangabe: None,
            suppress_wire: false,
        }
    }

    /// A **Verarbeitbarkeitsfehlermeldung** (29001, `BGM+313`) carrying the
    /// `ERC` code `error_code` and the free text `reason`.
    #[must_use]
    pub fn fehler(
        sender: &str,
        receiver: &str,
        orig_message_ref: &str,
        error_code: &str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            sender: sender.to_owned(),
            receiver: Some(receiver.to_owned()),
            pid: APERAK_PID_FEHLER,
            orig_message_ref: orig_message_ref.to_owned(),
            error_code: Some(error_code.to_owned()),
            reason: Some(reason.into()),
            ortsangabe: None,
            suppress_wire: false,
        }
    }

    /// Keep this entry off the wire — a positive Gas decision, which APERAK
    /// AHB 1.1 §2.3 answers with silence.
    #[must_use]
    pub fn off_the_wire(mut self) -> Self {
        self.suppress_wire = true;
        self
    }

    /// Whether this is a Fehlermeldung.
    #[must_use]
    pub fn is_fehler(&self) -> bool {
        self.error_code.is_some()
    }
}

// ── UTILMD ────────────────────────────────────────────────────────────────────

/// The payload of a `UTILMD` outbox entry — every field the UTILMD renderer
/// reads, under the name it reads it by.
///
/// One struct for every Anwendungsfall: which fields a Vorgang needs is the
/// Prüfschablone's question, and the renderer asks it. A field the profile
/// requires and the payload lacks is refused at render time, naming the field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UtilmdPayload {
    /// Prüfidentifikator.
    pub pid: u32,
    /// Sender MP-ID (our own).
    pub sender: String,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// Marktlokations-ID → `SG5 LOC+Z16` (Gas: `172`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo: Option<String>,
    /// Messlokations-ID → `SG5 LOC+Z17`, on the WiM MSB-Wechsel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub melo: Option<String>,
    /// MaBiS-Zählpunkt → `SG5 LOC+Z15`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mabis_zaehlpunkt: Option<String>,
    /// Overrides the per-PID `SG4 DTM` DE 2005 qualifier (55611).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dtm_qualifier: Option<String>,
    /// Process date, `YYYYMMDD` or `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_date: Option<String>,
    /// Document date; absent means the date of dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// Message reference; absent means derived from the causing event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1001 where the Anwendungsfall fixes one other than `E01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_code: Option<String>,
    /// `IDE+24` DE 7402; absent means the message reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vorgangsnummer: Option<String>,
    /// Second `SG4 DTM+158`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzierungsbeginn: Option<String>,
    /// Second `SG4 DTM+159`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzierungsende: Option<String>,
    /// `SG4 SG6 RFF+TN` — the request's `IDE+24`, on every answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_vorgangsnummer: Option<String>,
    /// `SG4 SG6 RFF+Z60`, on a Bestätigung Anmeldung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geplantes_produktpaket: Option<String>,
    /// `SG4 STS+7` DE 9013 element 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaktionsgrund: Option<String>,
    /// `SG4 STS+7` DE 9013 element 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG4 STS+E01` DE 9013 — required on every Antwort-PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_code: Option<String>,
    /// `STS+E01` DE 1131 — the Codeliste the code comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_codeliste: Option<String>,
    /// `SG4 STS+Z35` — the LFA's own code, restated on `A50`/`A57`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dritter_antwortcode: Option<String>,
    /// `STS+Z35` `C555` DE 9012.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dritter_referenz_lokation: Option<String>,
    /// The second DE 9013 of `STS+Z35`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dritter_objekt: Option<String>,
    /// `FTX+ACB` free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bemerkung: Option<String>,
    /// Strom `SG8 SEQ+Z79` Produktpaket · Gas `SG10 CCI+Z19`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzkreis: Option<String>,
    /// `SG8` Tranchengröße, on Geschäftsvorfall 3 (`ZW2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tranchengroesse: Option<Tranchengroesse>,
    /// `SG12 NAD+Z09` „Kunde des LF" — a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kunde_name: Option<String>,
    /// `NAD` DE 3045 — `Z01` Person, `Z02` Firma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kunde_namensformat: Option<String>,
    /// `SG12 NAD+Z04` Korrespondenzanschrift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kunde_anschrift: Option<Anschrift>,
    /// `SG8 SEQ+Z01 CCI` Gruppenzuordnung nach EnWG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kundengruppe: Option<String>,
    /// `SG8 SEQ+Z75 CCI+Z61` Grundlage nach EnFG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enfg_grundlage: Option<String>,
    /// `CAV` — the § of the EnFG privilege.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enfg_grund: Option<String>,
    /// `SG12 NAD+VY` — every beteiligter Marktpartner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub beteiligte_marktpartner: Vec<String>,
    /// `SG5 LOC` DE 3227, where the Vorgang names another object than the PID's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lokationstyp: Option<String>,
    /// `SG8 SEQ+Z98` — the MSB assigned to the Marktlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_msb: Option<ZugeordneterMsb>,
    /// `SG5 LOC+Z17` with its `SG8 SEQ+ZF3` — every Messlokation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messlokationen: Vec<UtilmdMesslokation>,
    /// A Listennachricht's members; non-empty selects that message shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positionen: Vec<ListenPosition>,
    /// `SG8 SEQ+Z22 RFF+AUU` Version der Zeitreihe, on a Liste.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeitreihen_version: Option<String>,
    /// The Liste's `IDE+Z01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listennummer: Option<String>,
    /// `SG6 RFF+TN` — the answered list's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_listennummer: Option<String>,
    /// `DTM+157` Bilanzierungsmonat, `CCYYMM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_period: Option<String>,
}

impl OutboundPayload for UtilmdPayload {
    const MESSAGE_TYPE: &'static str = "UTILMD";
}

/// The Tranchengröße: a bare string is the prozentuale Aufteilung; the object
/// names which of the three Produkteigenschaften it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tranchengroesse {
    /// The prozentuale Aufteilung.
    Prozent(String),
    /// `art` (`prozent`, `aufteilungsfaktor`, `technische_ressourcen`) and value.
    Eigenschaft {
        /// Which Produkteigenschaft; absent means `prozent`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        art: Option<String>,
        /// The value.
        wert: String,
    },
}

/// A postal address — `SG12 NAD` `C059`/`C080` components.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anschrift {
    /// Street and house number.
    #[serde(default)]
    pub strasse: String,
    /// Postal code.
    #[serde(default)]
    pub plz: String,
    /// Town.
    #[serde(default)]
    pub ort: String,
    /// Country code; absent means `DE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<String>,
}

/// A `ZugeordneterMsb` — the four codes `SG10 CCI+++ZB3` needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZugeordneterMsb {
    /// MP-ID of the MSB.
    pub mp_id: String,
    /// Rolle; absent means grundzuständig.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolle: Option<String>,
    /// Grundlage; absent means Vertrag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grundlage: Option<String>,
    /// MP-ID of the grundzuständiger MSB; absent means `mp_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gmsb_mp_id: Option<String>,
}

/// One Messlokation of a Marktlokation, with its MSB.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtilmdMesslokation {
    /// Messlokations-ID.
    pub melo_id: String,
    /// Its MSB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msb: Option<ZugeordneterMsb>,
}

/// One member of a UTILMD Listennachricht.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenPosition {
    /// The disputed Marktlokation.
    pub malo: String,
    /// Its `STS+E01` code.
    pub antwort_code: String,
    /// `IDE+24`; absent means minted from the list number and position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vorgangsnummer: Option<String>,
    /// `SG6 RFF+TN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_vorgangsnummer: Option<String>,
}

// ── MaloIdentified ────────────────────────────────────────────────────────────

/// The payload of a `MaloIdentified` ERP notification: the MaLo-Identifikation
/// answered a supplier's query, and the ERP continues the Anmeldung with it.
///
/// Not a BO4E object — it names the transaction and the MaLo it resolved to —
/// so the entry carries no `payload_schema`, and the CloudEvent no
/// `dataschema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaloIdentifiedPayload {
    /// The MaLo-Identifikation transaction the answer belongs to.
    pub tx_id: String,
    /// The Marktlokations-ID the query resolved to.
    pub malo_id: String,
    /// MP-ID of the Netzbetreiber the MaLo is assigned to.
    pub nb_mp_id: String,
    /// MP-ID of the supplier that asked.
    pub sender_market_partner_id: String,
    /// Tenant identifier — data-isolation key written to every database row.
    pub tenant_id: String,
}

impl OutboundPayload for MaloIdentifiedPayload {
    const MESSAGE_TYPE: &'static str = "MaloIdentified";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two constructors pair the Anwendungsfall with the presence of an
    /// error code, which is what decides `BGM`.
    #[test]
    fn the_anwendungsfall_follows_the_error_code() {
        let ok = AperakPayload::anerkennung("9900000000001", "9900000000002", "UNH1");
        assert_eq!(ok.pid, APERAK_PID_ANERKENNUNG);
        assert!(!ok.is_fehler());
        let err = AperakPayload::fehler("9900000000001", "9900000000002", "UNH1", "Z29", "x");
        assert_eq!(err.pid, APERAK_PID_FEHLER);
        assert!(err.is_fehler());
    }

    /// An entry round-trips through the stored JSON, and a row written with an
    /// extra key still reads.
    #[test]
    fn the_payload_round_trips_and_reads_tolerantly() {
        let p = AperakPayload::fehler("a", "b", "UNH1", "Z29", "kaputt").off_the_wire();
        let json = serde_json::to_value(&p).expect("serialise");
        assert_eq!(AperakPayload::from_payload(&json).expect("read"), p);
        let mut extra = json;
        extra["positive"] = serde_json::Value::Bool(false);
        assert_eq!(AperakPayload::from_payload(&extra).expect("tolerant"), p);
    }
}
