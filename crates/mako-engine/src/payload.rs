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
//! An outbox row is written by one build and may be drained by the next, so a
//! field is an `Option` or carries `#[serde(default)]`: a row that lacks a
//! field a later build added still reads.
//!
//! **No struct admits a key it does not declare** (`#[serde(deny_unknown_fields)]`).
//! A producer that builds its payload with `serde_json::json!` instead of the
//! struct can spell a key no renderer reads, and serde would drop it in
//! silence — the GeLi Gas Änderung Antworten wrote their Antwortcode under
//! `pruefung`, and no `SG4 STS+E01` reached the wire. Refused, the key fails
//! the render naming itself, and [`undeclared_key`] lets
//! [`PendingOutbox::new`](crate::outbox::PendingOutbox::new) refuse it where
//! it is written.

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
#[serde(deny_unknown_fields)]
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
    /// no Anerkennungsmeldung (APERAK AHB 1.1 Kap. 2.3), so the entry reaches the
    /// ERP and never the wire.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppress_wire: bool,
    /// Every further Verarbeitbarkeitsfehler of the Geschäftsvorfall beyond the
    /// first (`error_code`), each its own `SG4 ERC` group — APERAK AHB 1.1
    /// Kap. 2.1.1 (S. 10): „Es sind alle AHB-Fehler anzugeben"; the APERAK MIG
    /// lets `SG4` repeat 99999 times.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weitere_fehler: Vec<AperakFehler>,
    /// `BGM` DE 1004 of the acknowledged message — `SG2 RFF+AGO` on an
    /// Anerkennungsmeldung (APERAK AHB 1.1 Kap. 3, Muss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig_document_number: Option<String>,
    /// `DTM+137` of the acknowledged message, `CCYYMMDDHHMM` UTC — `SG2
    /// DTM+171` Referenzdatum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orig_document_date: Option<String>,
}

/// One further `SG4 ERC` group of a Fehlermeldung.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AperakFehler {
    /// `ERC` DE 9321.
    pub error_code: String,
    /// `FTX+ABO` free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `SG5 FTX+Z02` Ortsangabe des AHB-Fehlers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ortsangabe: Option<String>,
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
            weitere_fehler: Vec::new(),
            orig_document_number: None,
            orig_document_date: None,
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
            weitere_fehler: Vec::new(),
            orig_document_number: None,
            orig_document_date: None,
        }
    }

    /// Name the acknowledged message's `BGM` Dokumentennummer and its
    /// `DTM+137` Dokumentendatum (`CCYYMMDDHHMM` UTC), which `SG2 RFF+AGO` and
    /// `SG2 DTM+171` carry.
    #[must_use]
    pub fn referencing(mut self, document_number: &str, document_date: Option<String>) -> Self {
        self.orig_document_number = Some(document_number.to_owned());
        self.orig_document_date = document_date;
        self
    }

    /// Keep this entry off the wire — a positive Gas decision, which APERAK
    /// AHB 1.1 Kap. 2.3 answers with silence.
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
#[serde(deny_unknown_fields)]
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
    /// `SG4 DTM+206` Geräteausbaudatum — the 55007/55008 with `STS+7++Z33`
    /// Auszug wegen Stilllegung (UTILMD AHB Strom 2.2 Kap. 8.10, S. 189,
    /// Bedingung `[79]`), a German calendar day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geraeteausbaudatum: Option<String>,
    /// Second `SG4 DTM+92` „Datum Vertragsbeginn", beside a `157` process
    /// date — a GeLi Gas Antwort auf Anfrage restates both dates of the
    /// Anfrage (UTILMD AHB Gas 1.2 Kap. 5.12.1, Bedingung `[528]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertragsbeginn: Option<String>,
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
    /// `SG8 SEQ+Z01 CCI+Z36` Versorgungsart der Marktlokation (`ZC9`/`ZD0`/
    /// `ZE3`), in the same block as the Gruppenzuordnung — the Bestätigung
    /// EoG 55014.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versorgungsart: Option<String>,
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
    /// The members of a list this participant distributes (55065 / 55201);
    /// non-empty selects the Listennachricht shape as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listenmitglieder: Vec<ListenMitglied>,
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
    /// `SG4 SG6 RFF+AAV` „Nummer der Anfrage" — the `BGM` DE 1004 of the
    /// ORDERS a Stammdaten-Antwort answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anfrage_referenz: Option<String>,
    /// Stammdaten the sender supplies whole — the answer to a
    /// Geschäftsdatenanfrage. Empty everywhere else.
    #[serde(default, skip_serializing_if = "Stammdaten::is_empty")]
    pub stammdaten: Stammdaten,
}

impl OutboundPayload for UtilmdPayload {
    const MESSAGE_TYPE: &'static str = "UTILMD";
}

/// Stammdaten a Vorgang carries whole: every `SG5 LOC`, `SG6` segment, `SG8`
/// block and `SG12` segment, as the sender holds them.
///
/// A Stammdaten-Antwort is the Netzbetreiber's master data of a Lokation —
/// UTILMD AHB Strom 2.2 Kap. 9.5 gives 55035, 55095 and 55060 several dozen
/// groups each — and its content is the sender's record, not something a
/// workflow derives. It is carried segment by segment and the AHB profile
/// checks what renders.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stammdaten {
    /// `SG5 LOC` — `(DE 3227 qualifier, DE 3225 ID)`, in the MIG's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lokationen: Vec<StammdatenLokation>,
    /// `SG6` segments — each `RFF` and the `DTM`s of its group, in order:
    /// the Termine der Marktlokation (`RFF+Z18` with `DTM+Z21`/`Z09`/`Z22`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sg6: Vec<StammdatenSegment>,
    /// `SG8` blocks, each opened by its `SEQ`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bloecke: Vec<StammdatenBlock>,
    /// `SG12` segments — each `NAD` and the segments of its group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sg12: Vec<StammdatenSegment>,
}

impl Stammdaten {
    /// `true` when nothing is carried.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lokationen.is_empty()
            && self.sg6.is_empty()
            && self.bloecke.is_empty()
            && self.sg12.is_empty()
    }
}

/// One `SG5 LOC`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StammdatenLokation {
    /// DE 3227 — `Z16` Marktlokation, `Z17` Messlokation, `Z18` Netzlokation, …
    pub qualifier: String,
    /// DE 3225 — the Lokations-ID.
    pub id: String,
}

/// One `SG8` block: its `SEQ` DE 1229 and the segments under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StammdatenBlock {
    /// `SEQ` DE 1229 — `ZD5`, `Z98`, `ZE1`, …
    pub seq: String,
    /// The block's `RFF`, `PIA`, `SG9 QTY`, `SG10 CCI`/`CAV`, in order.
    #[serde(default)]
    pub segmente: Vec<StammdatenSegment>,
}

/// One segment: its tag and its data elements, each a list of components.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StammdatenSegment {
    /// Segment tag.
    pub tag: String,
    /// Data elements in position order, each its components in order.
    #[serde(default)]
    pub elemente: Vec<Vec<String>>,
}

/// The Tranchengröße: a bare string is the prozentuale Aufteilung; the object
/// names which of the three Produkteigenschaften it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct UtilmdMesslokation {
    /// Messlokations-ID.
    pub melo_id: String,
    /// Its MSB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msb: Option<ZugeordneterMsb>,
}

/// One member of a UTILMD Listennachricht.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// One Marktlokation (or Tranche) of a Clearingliste the Netzbetreiber
/// distributes, with its bilanzierungsrelevante Daten carried whole.
///
/// UTILMD AHB Strom 2.2 Kap. 13.2 (S. 1114–1121) names, per `IDE+24`: the
/// Bilanzierungsbeginn `DTM+158` (Muss) and -ende `DTM+159` (Soll `[14]`), the
/// Marktlokation `SG5 LOC+Z16` and the Tranche `LOC+Z21`, and `SG8` blocks
/// carrying the quantities, Regelzone, Bilanzkreis, Bilanzierungsgebiet and
/// Zeitreihentyp. Those blocks are the sender's record; the AHB profile checks
/// what renders.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenMitglied {
    /// `SG5 LOC+Z16` — the Marktlokation.
    pub malo: String,
    /// `SG5 LOC+Z21` — the Tranche, when the list names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tranche: Option<String>,
    /// `IDE+24`; absent means minted from the list number and position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vorgangsnummer: Option<String>,
    /// `SG4 DTM+158` Bilanzierungsbeginn, the UTC instant `CCYYMMDDHHMM`.
    pub bilanzierungsbeginn: String,
    /// `SG4 DTM+159` Bilanzierungsende, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzierungsende: Option<String>,
    /// The member's `SG8` blocks as the sender holds them.
    #[serde(default, skip_serializing_if = "Stammdaten::is_empty")]
    pub stammdaten: Stammdaten,
}

// ── MaloIdentified ────────────────────────────────────────────────────────────

/// The payload of a `MaloIdentified` ERP notification: the MaLo-Identifikation
/// answered a supplier's query, and the ERP continues the Anmeldung with it.
///
/// Not a BO4E object — it names the transaction and the MaLo it resolved to —
/// so the entry carries no `payload_schema`, and the CloudEvent no
/// `dataschema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

// ── Shared parts of the ORDERS family ─────────────────────────────────────────

/// An Ansprechpartner — `CTA` with one `COM` per channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ansprechpartner {
    /// `CTA` DE 3412 — the name.
    pub name: String,
    /// The `COM` channels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kommunikation: Vec<Kommunikationsweg>,
}

/// One `COM` channel of an [`Ansprechpartner`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kommunikationsweg {
    /// `email`, `telefon`, `telefax`, `weiteres_telefon` or `handy` — DE 3155
    /// `EM`/`TE`/`FX`/`AJ`/`AL`.
    pub art: String,
    /// DE 3148 — the address or number.
    pub adresse: String,
}

// ── ORDERS ────────────────────────────────────────────────────────────────────

/// The payload of an `ORDERS` outbox entry — every field the ORDERS renderer
/// reads. Which a column needs is the renderer's question; a Muss field the
/// payload lacks is refused there, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdersPayload {
    /// Prüfidentifikator → `SG1 RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's MP-ID for the PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// The Sparte — picks the sender between two roles that share a PID, and
    /// the day boundary of a process date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparte: Option<crate::types::Sparte>,
    /// `UNH` DE 0062 source; absent means derived from the causing event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1001 where the column fixes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_code: Option<String>,
    /// `BGM` DE 1004 — the Belegnummer the answer echoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// `SG2 LOC+172` Meldepunkt, opened by `NAD+DP`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The reference an ESA order correlates by (`RFF+AAG` / `RFF+ACW`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// `DTM+203` Ausführungsdatum, a German calendar day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ausfuehrungsdatum: Option<String>,
    /// `IMD` DE 7081 — the Leistung or Abonnement the order names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abonnement: Option<String>,
    /// First day of the 17103 Zeitspanne, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeitspanne_von: Option<String>,
    /// Last day of the 17103 Zeitspanne, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeitspanne_bis: Option<String>,
    /// 17001 `SG1 RFF+AAG` — the Angebotsnummer ordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angebotsnummer: Option<String>,
    /// 17001 `SG5 CTA`/`COM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansprechpartner: Option<Ansprechpartner>,
    /// 17001 — one `SG29 LIN` with `SG34 RFF+Z03` per Angebotsposition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positionen: Vec<String>,
    /// 17009 `IMD` — `Z58`/`Z59`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub umfang: Option<String>,
    /// 17009 — one `SG29 LIN` per Gerät.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub geraete: Vec<OrdersGeraet>,
    /// 17004 `SG29 IMD` — the Grund.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grund: Option<String>,
    /// 17004 `SG29 DTM+7` — the Wertzeitpunkt, a German calendar day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wertzeitpunkt: Option<String>,
    /// 17011 — one `SG29 LIN+Z19` with `PIA+5+<code>:Z11` per Produkt of
    /// Codeliste der Konfigurationen Kap. 7.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produkte: Vec<String>,
    /// 17011 `SG3 RFF+Z18` — the Marktlokation of a weitere
    /// Energieflussrichtung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marktlokation: Option<String>,
    /// 17011 `SG2 NAD+DEB` — the MSB der Marktlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msb_marktlokation: Option<String>,
    /// MaBiS `DTM+273` Betrachtungszeitintervall, `CCYYMM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeitintervall: Option<String>,
    /// MaBiS `SG34 RFF+AUU` — the Version of the Summenzeitreihe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// MaBiS `SG38 LOC+172` — the Zählpunkt of the list asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meldepunkt: Option<String>,
    /// MaBiS `SG38 LOC+237` — the Bilanzkreis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzkreis: Option<String>,
    /// MaBiS `SG2 LOC+107` — the Bilanzierungsgebiet, beside the receiver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilanzierungsgebiet: Option<String>,
    /// 17201 `SG30 CCI` — the Profilgruppe (`Z02`–`Z05`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profilgruppe: Option<String>,
    /// 17211 `FTX+AFH` — the `E_0100` Prüfschritt code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reklamation_code: Option<String>,
    /// 17211 `SG1 RFF+ACW` — the reclaimed MSCONS' `BGM` DE 1004.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_nachricht: Option<String>,
}

impl OutboundPayload for OrdersPayload {
    const MESSAGE_TYPE: &'static str = "ORDERS";
}

/// One Gerät of a 17009 Gerätewechselabsicht.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdersGeraet {
    /// `SG30 CCI` — the Geräteart.
    pub geraeteart: String,
    /// `SG34 RFF+Z09` — every Gerätenummer.
    #[serde(default)]
    pub geraetenummern: Vec<String>,
}

// ── REQOTE ────────────────────────────────────────────────────────────────────

/// The payload of a `REQOTE` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReqotePayload {
    /// Prüfidentifikator → `SG1 RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `LOC+172` Meldepunkt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// 35003 `DTM+76` Wunschtermin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wunschtermin: Option<String>,
    /// 35003 `CTA` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// 35003 `COM` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_comm: Option<String>,
    /// `FTX+ACB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bemerkung: Option<String>,
    /// 35003 `SG27 PIA+5` — a Messprodukt of Codeliste der Konfigurationen
    /// Kap. 4.6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messprodukt: Option<String>,
    /// Kap. 4.6.2 delivery target and Schwellwerte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smgw: Option<SmgwZiel>,
}

impl OutboundPayload for ReqotePayload {
    const MESSAGE_TYPE: &'static str = "REQOTE";
}

/// The SM-PKI delivery target of a Kap. 4.6.2 Messprodukt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmgwZiel {
    /// `FTX+Z17` — IPv4 URI.
    pub uri_ipv4: String,
    /// `FTX+Z17` — IPv6 URI.
    pub uri_ipv6: String,
    /// `FTX+Z24` — Zertifikatsaussteller.
    pub zertifikat_aussteller: String,
    /// `FTX+Z23` — Zertifikatsnutzer.
    pub zertifikat_nutzer: String,
    /// `SG28 CCI+Z60` thresholds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schwellwerte: Vec<Schwellwert>,
}

/// One `SG28 CCI+Z60` Schwellwert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schwellwert {
    /// The position code.
    pub position_code: String,
    /// Upper bound.
    pub oberer: String,
    /// Lower bound.
    pub unterer: String,
}

// ── ORDCHG ────────────────────────────────────────────────────────────────────

/// The payload of an `ORDCHG` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdchgPayload {
    /// Prüfidentifikator → `SG1 RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// The ESA reference, under the PID's own qualifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// `SG1 RFF+ON` — the ORDERS changed or cancelled. Muss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_reference: Option<String>,
}

impl OutboundPayload for OrdchgPayload {
    const MESSAGE_TYPE: &'static str = "ORDCHG";
}

// ── ORDRSP ────────────────────────────────────────────────────────────────────

/// The payload of an `ORDRSP` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdrspPayload {
    /// Prüfidentifikator → `SG1 RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1001 where the column fixes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_code: Option<String>,
    /// `BGM` DE 1004.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// `DTM+137`; absent means the instant of dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// `SG1 RFF+ON` — the answered ORDERS' `BGM` DE 1004.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_reference: Option<String>,
    /// The ESA reference, under the PID's own qualifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// `SG2 AJT` DE 4465 — the Antwortcode, Muss on every answering PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_code: Option<String>,
    /// `SG2 AJT` DE 1082 — the Codeliste the code comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_codeliste: Option<String>,
    /// `SG2 FTX+AAP` — the problem the tree requires named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_freitext: Option<String>,
    /// `IMD` DE 7081 — the Leistung the answer echoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leistung: Option<String>,
    /// 19011/19012 `IMD` — the Abonnement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abonnement: Option<String>,
    /// `DTM+203` Ausführungsdatum, a German calendar day of `sparte`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ausfuehrungsdatum: Option<String>,
    /// 19003/19004 `DTM+Z02` verschobener Abmeldetermin — the day up to which
    /// the MSBA is obliged to continue, a German calendar day of `sparte`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abmeldetermin: Option<String>,
    /// 19130 with `A06`: `DTM+292` the geplanter Behebungszeitpunkt, a UTC
    /// instant `CCYYMMDDHHMM` (ORDRSP AHB 1.1b Kap. 4.7, S. 37, `[46]`–`[48]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behebungszeitpunkt: Option<String>,
    /// `SG1 RFF+TN` — the reclaimed Vorgang's `IDE` DE 7402, beside
    /// `korrelation_ref` (`RFF+ACW`) on the 19123 (Kap. 4.5, S. 33–35).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_vorgangsnummer: Option<String>,
    /// The Sparte whose day `ausfuehrungsdatum` / `abmeldetermin` begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparte: Option<crate::types::Sparte>,
    /// `SG6 CTA`/`COM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansprechpartner: Option<Ansprechpartner>,
    /// `SG3 NAD+VY` — the Eigentümer's MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eigentuemer: Option<String>,
    /// `SG3 NAD+Z22` — the Lieferadresse für Altgeräte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lieferadresse: Option<Lieferadresse>,
}

impl OutboundPayload for OrdrspPayload {
    const MESSAGE_TYPE: &'static str = "ORDRSP";
}

/// `SG3 NAD+Z22` — a Lieferadresse.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lieferadresse {
    /// Name.
    pub name: String,
    /// Street and house number.
    pub strasse: String,
    /// Town.
    pub ort: String,
    /// Postal code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postleitzahl: Option<String>,
    /// Country code.
    pub land: String,
}

// ── QUOTES ────────────────────────────────────────────────────────────────────

/// The payload of a `QUOTES` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotesPayload {
    /// Prüfidentifikator → `SG1 RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1004 — the Angebotsnummer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// `DTM+137`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// `LOC+172`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// `SG1 RFF+AAV` — the REQOTE this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// `DTM+273` Bindungsfrist, in Tagen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindungsfrist_tage: Option<u32>,
    /// Ablehnungsgrund — present exactly on an Ablehnung der Anfrage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `SG4 CUX`; absent means `EUR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// `DTM+469` frühester Start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fruehester_start: Option<String>,
    /// `CTA` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// `COM` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_comm: Option<String>,
    /// `SG27 PIA+5` — the offered Messprodukt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messprodukt: Option<String>,
    /// `PIA+Z02` per Artikel-ID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artikel_ids: Vec<String>,
    /// OBIS-Kennzahlen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub obis: Vec<String>,
    /// `SG31 PRI+CAL` per price.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preise: Vec<QuotesPreis>,
}

impl OutboundPayload for QuotesPayload {
    const MESSAGE_TYPE: &'static str = "QUOTES";
}

/// One `SG31 PRI+CAL`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotesPreis {
    /// The amount, as a decimal string.
    pub betrag: String,
    /// `Z01` Einrichtungs-, `Z02` Transaktions-, `Z03` Betriebspreis; absent
    /// means `Z01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Base unit; absent means `DAY` on `Z03`, `H87` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub einheit: Option<String>,
}

// ── IFTSTA ────────────────────────────────────────────────────────────────────

/// The payload of an `IFTSTA` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IftstaPayload {
    /// Prüfidentifikator → `SG15 RFF+Z13`; one with no renderer arm is
    /// refused.
    pub pid: u32,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1004.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// `DTM+137`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// `SG15 STS` DE 4405 where the Anwendungsfall admits more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sts_code: Option<String>,
    /// `SG15 STS` DE 9013 — the Prüfschritt code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_code: Option<String>,
    /// `SG15 STS` DE 1131 — the Codeliste or EBD the code comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_codeliste: Option<String>,
    /// `SG15 RFF+ACW` — the message answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_nachricht: Option<String>,
    /// `SG15 RFF+ADY` — the Vorgang answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz_vorgang: Option<String>,
    /// `SG15 DTM+293` — the takeover day, begun in `sparte`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zuordnungsbeginn: Option<String>,
    /// The Sparte whose day `zuordnungsbeginn` begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparte: Option<crate::types::Sparte>,
    /// `SG14 LOC+172` — the Messlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub melo: Option<String>,
    /// `SG15 RFF+AVE` — the Marktlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo: Option<String>,
    /// `SG15 DTM+76` — the geplanter Umstellungszeitpunkt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub umstellungszeitpunkt: Option<String>,
    /// `SG15 RFF+AGI` — the Bestellung's Belegnummer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// `SG15 DTM+93` — the Vertragsende.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beendigung_zum: Option<String>,
    /// `SG25 FTX+ACB` — the free text an Anwendungsfall requires beside a
    /// code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bemerkung: Option<String>,
}

impl OutboundPayload for IftstaPayload {
    const MESSAGE_TYPE: &'static str = "IFTSTA";
}

// ── INSRPT ────────────────────────────────────────────────────────────────────

/// The payload of an `INSRPT` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsrptPayload {
    /// Prüfidentifikator → `BGM` DE 1004 and `RFF+Z13`.
    pub pid: u32,
    /// `SG8 LOC+172` — the Messlokation.
    pub melo: String,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `DTM+137`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// `SG7 LIN` Positionsnummer; absent means `1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// `SG7 STS+Z06` Gerätestatus; absent means `Z11` gestört.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<String>,
    /// `SG5 CTA` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// `SG5 COM` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_comm: Option<String>,
    /// `SG3 DOC` reference; absent means the PID under `21`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_reference: Option<String>,
}

impl OutboundPayload for InsrptPayload {
    const MESSAGE_TYPE: &'static str = "INSRPT";
}

// ── INVOIC ────────────────────────────────────────────────────────────────────

/// The payload of an `INVOIC` outbox entry: the message itself, built and
/// checked against its Prüfschablone before the issuing command recorded it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvoicPayload {
    /// The INVOIC, `UNH` … `UNT`, as EDIFACT text.
    pub nachricht: String,
    /// Sender MP-ID — `NAD+MS`, `UNB` DE 0004.
    pub sender: String,
    /// Receiver MP-ID — `NAD+MR`, `UNB` DE 0010.
    pub receiver: String,
}

impl OutboundPayload for InvoicPayload {
    const MESSAGE_TYPE: &'static str = "INVOIC";
}

// ── REMADV ────────────────────────────────────────────────────────────────────

/// The payload of a `REMADV` outbox entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemadvPayload {
    /// Prüfidentifikator → `RFF+Z13`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `BGM` DE 1004 — the Avisnummer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// `BGM` DE 1001.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_code: Option<String>,
    /// `DTM+137`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// `SG4 CUX` DE 6345; absent means `EUR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waehrung: Option<String>,
    /// `SG5` — the answered invoice and its amounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rechnungsbezug: Option<Rechnungsbezug>,
    /// `SG7 AJT` DE 4465 — the Antwortcode of an Abweisung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_code: Option<String>,
    /// `AJT` DE 1082 — the EBD the codes come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort_codeliste: Option<String>,
    /// `SG7 FTX+ABO` beside the Kopf-`AJT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ablehnungsgrund: Option<String>,
    /// Every Befund of the walk; those naming a Positionsnummer ride `SG10`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antwort_befunde: Vec<RemadvBefund>,
}

impl OutboundPayload for RemadvPayload {
    const MESSAGE_TYPE: &'static str = "REMADV";
}

/// `SG5` of a REMADV — the invoice answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rechnungsbezug {
    /// `DOC` DE 1001.
    pub dokumentenart: String,
    /// `DOC` DE 1004.
    pub rechnungsnummer: String,
    /// `MOA+9`.
    pub faelliger_betrag: String,
    /// `MOA+12`.
    pub ueberweisungsbetrag: String,
    /// `DTM+137` — the invoice's document date.
    pub rechnungsdatum: String,
}

/// One Befund of a REMADV Prüfung.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemadvBefund {
    /// `AJT` DE 4465.
    pub code: String,
    /// The Ebene the Befund was found on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ebene: Option<String>,
    /// The Positionsnummer of a position-level Befund — `SG10 DLI`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positionsnummer: Option<u16>,
    /// `SG12 FTX` explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// ── MSCONS ────────────────────────────────────────────────────────────────────

/// The payload of an `MSCONS` outbox entry. The Prüfidentifikator selects the
/// shape: a Summenzeitreihe (13003/13023), Arbeit and Leistungsmaxima
/// (13015/13016/13019), or Werte nach Typ 2 (13027).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsconsPayload {
    /// Prüfidentifikator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Sender MP-ID; absent means the deployment's primary MP-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_mp_id: Option<String>,
    /// Receiver MP-ID; absent means the outbox entry's recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_mp_id: Option<String>,
    /// `UNH` DE 0062 source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    /// `SG1 RFF+AGI` — the Bestellung answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub korrelation_ref: Option<String>,
    /// Summenzeitreihe: `SG6 LOC+172` — the MaBiS-Zählpunkt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mabis_zp_id: Option<String>,
    /// Summenzeitreihe: the Bilanzierungsmonat, `CCYYMM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balancing_period: Option<String>,
    /// Summenzeitreihe: the Version, `CCYYMMDDHHMMSSZZZ`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Summenzeitreihe: `SG9 PIA+5` OBIS-Kennzahl.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obis_code: Option<String>,
    /// Summenzeitreihe: one entry per settlement slot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intervals: Vec<MsconsInterval>,
    /// 13015/13016/13019/13027: `SG6 LOC+172` — the Marktlokation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<String>,
    /// 13015/13016/13019: the Energiemenge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arbeit: Option<MsconsWert>,
    /// 13016/13015: up to two Leistungsmaxima.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leistungsmaxima: Vec<MsconsWert>,
    /// 13019: `SG10 STS+10` Messklassifizierung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messklassifizierung: Option<String>,
    /// 13027: the Werte nach Typ 2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<MsconsRead>,
    /// 13017: `SG7 RFF+MG` — the Zähler the Stände were read from, beside
    /// the Messlokation in `SG6 LOC+172`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geraetenummer: Option<String>,
    /// 13017: `SG7 RFF+AGK` — the Konfigurations-ID of values from an iMS,
    /// which then name no Lokation in `SG6 LOC+172` (MSCONS AHB 3.2 Kap.
    /// 6.3.1, S. 23–25, `[131]`/`[132]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub konfigurations_id: Option<String>,
    /// 13017: the Zählerstände, one `SG9 LIN` each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zaehlerstaende: Vec<MsconsZaehlerstand>,
}

/// One Zählerstand of an MSCONS 13017 (MSCONS AHB 3.2 Kap. 6.3.1, S. 25–29).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsconsZaehlerstand {
    /// `SG9 PIA+5` — the OBIS-Kennzahl.
    pub obis_code: String,
    /// `SG10 QTY` DE 6060, as a decimal string.
    pub quantity: String,
    /// `SG10 DTM+7` Gültigkeitsdatum — the German calendar day whose
    /// beginning the Stand holds for (`[UB1]`).
    pub gueltig: String,
    /// `SG10 DTM+9` Ablesedatum, a German calendar day — on a wahrer Wert of
    /// a Messlokation only (`[93]`, `[131]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ablesedatum: Option<String>,
    /// A substitute value — `QTY` DE 6063 `67`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ersatzwert: bool,
    /// `STS+Z32` Ersatzwertbildungsverfahren, Muss on an Ersatzwert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ersatzwert_verfahren: Option<String>,
    /// `STS+Z40` Grund der Ersatzwertbildung, Muss on an Ersatzwert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ersatzwert_grund: Option<String>,
}

impl OutboundPayload for MsconsPayload {
    const MESSAGE_TYPE: &'static str = "MSCONS";
}

/// One settlement slot of a Summenzeitreihe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsconsInterval {
    /// Slot start.
    pub from: String,
    /// Slot end.
    pub to: String,
    /// The quantity, kWh, as a decimal string.
    pub quantity_kwh: String,
}

/// An Energiemenge or a Leistungsmaximum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsconsWert {
    /// The value, as a decimal string.
    pub quantity: String,
    /// Energiemenge: period start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Energiemenge: period end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Leistungsmaximum: the period it fell in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// `610` (`CCYYMM`, the default) or `102` (`CCYYMMDD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_format: Option<String>,
    /// `QTY` DE 6411, where stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// OBIS-Kennzahl; absent means Wirkarbeit resp. Leistungsmaximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obis: Option<String>,
    /// A substitute value — `QTY` DE 6063 `67`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ersatzwert: bool,
    /// `STS+Z32` Ersatzwertbildungsverfahren.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ersatzwert_verfahren: Option<String>,
    /// `STS+Z40` Grund der Ersatzwertbildung.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ersatzwert_grund: Option<String>,
}

/// One interval value of a Werte-nach-Typ-2 delivery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsconsRead {
    /// The quantity, kWh, as a decimal string.
    pub quantity_kwh: String,
    /// Interval start.
    pub dtm_from: String,
    /// Interval end.
    pub dtm_to: String,
    /// OBIS-Kennzahl — groups the values into line items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obis_code: Option<String>,
    /// A substitute value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ersatzwert: bool,
}

/// The serde error naming a key `payload` carries that the typed payload of
/// `message_type` does not declare — `None` when every key is declared, or
/// when `message_type` has no typed payload here.
///
/// Only the unknown-key refusal is reported. A payload that lacks a field is
/// another question: some entries are intent-only and the renderer names what
/// is missing when it renders.
#[must_use]
pub fn undeclared_key(message_type: &str, payload: &serde_json::Value) -> Option<String> {
    fn refusal<P: OutboundPayload>(payload: &serde_json::Value) -> Option<String> {
        let text = P::from_payload(payload).err()?.to_string();
        text.contains("unknown field").then_some(text)
    }
    match message_type {
        UtilmdPayload::MESSAGE_TYPE => refusal::<UtilmdPayload>(payload),
        AperakPayload::MESSAGE_TYPE => refusal::<AperakPayload>(payload),
        MaloIdentifiedPayload::MESSAGE_TYPE => refusal::<MaloIdentifiedPayload>(payload),
        OrdersPayload::MESSAGE_TYPE => refusal::<OrdersPayload>(payload),
        ReqotePayload::MESSAGE_TYPE => refusal::<ReqotePayload>(payload),
        OrdchgPayload::MESSAGE_TYPE => refusal::<OrdchgPayload>(payload),
        OrdrspPayload::MESSAGE_TYPE => refusal::<OrdrspPayload>(payload),
        QuotesPayload::MESSAGE_TYPE => refusal::<QuotesPayload>(payload),
        IftstaPayload::MESSAGE_TYPE => refusal::<IftstaPayload>(payload),
        InsrptPayload::MESSAGE_TYPE => refusal::<InsrptPayload>(payload),
        InvoicPayload::MESSAGE_TYPE => refusal::<InvoicPayload>(payload),
        RemadvPayload::MESSAGE_TYPE => refusal::<RemadvPayload>(payload),
        MsconsPayload::MESSAGE_TYPE => refusal::<MsconsPayload>(payload),
        _ => None,
    }
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

    /// An entry round-trips through the stored JSON, and a row that lacks a
    /// field a later build added still reads.
    #[test]
    fn the_payload_round_trips_and_reads_a_row_without_a_newer_field() {
        let p = AperakPayload::fehler("a", "b", "UNH1", "Z29", "kaputt").off_the_wire();
        let json = serde_json::to_value(&p).expect("serialise");
        assert_eq!(AperakPayload::from_payload(&json).expect("read"), p);
        let mut older = json;
        older
            .as_object_mut()
            .expect("an object")
            .remove("suppress_wire");
        let read = AperakPayload::from_payload(&older).expect("a missing field defaults");
        assert!(!read.suppress_wire);
    }

    /// A key no struct declares is refused, not dropped: a producer that wrote
    /// the GeLi Gas Antwortcode under `pruefung` sent no `SG4 STS+E01`, and
    /// serde said nothing. The nested structs refuse in the same way.
    #[test]
    fn an_undeclared_key_is_refused_at_every_level() {
        let utilmd = serde_json::json!({
            "pid": 44_115, "sender": "9800000000001", "pruefung": "E15",
        });
        let refused = undeclared_key("UTILMD", &utilmd).expect("refused");
        assert!(refused.contains("pruefung"), "{refused}");
        let nested = serde_json::json!({
            "pid": 44_142, "sender": "9800000000001",
            "stammdaten": { "lokationen": [{ "qualifier": "Z16", "id": "x", "typ": "y" }] },
        });
        assert!(undeclared_key("UTILMD", &nested).is_some());
        let tranche = serde_json::json!({
            "pid": 55_001, "sender": "9900000000001",
            "tranchengroesse": { "wert": "50", "einheit": "%" },
        });
        assert!(UtilmdPayload::from_payload(&tranche).is_err());
        let aperak = serde_json::json!({
            "sender": "a", "pid": 29_001, "orig_message_ref": "UNH1", "positive": false,
        });
        assert!(undeclared_key("APERAK", &aperak).is_some());
        // A declared payload passes, and a payload missing a field is the
        // renderer's to name.
        assert_eq!(
            undeclared_key(
                "UTILMD",
                &serde_json::json!({ "pid": 44_115, "sender": "x" })
            ),
            None
        );
        assert_eq!(undeclared_key("UTILMD", &serde_json::json!({})), None);
        assert_eq!(undeclared_key("ProcessCompleted", &utilmd), None);
    }

    /// Every rendered message type refuses a key its renderer does not read —
    /// the WiM ORDRSP answers carried `antwort_tree`, `orig_message_ref` and
    /// `abmeldetermin`, which no renderer read and none reached the wire.
    #[test]
    fn every_rendered_message_type_refuses_an_unread_key() {
        for (message_type, declared) in [
            (
                "ORDERS",
                serde_json::json!({ "pid": 17_009, "location": "x" }),
            ),
            (
                "REQOTE",
                serde_json::json!({ "pid": 35_003, "location": "x" }),
            ),
            (
                "ORDCHG",
                serde_json::json!({ "pid": 39_000, "order_reference": "x" }),
            ),
            (
                "ORDRSP",
                serde_json::json!({ "pid": 19_003, "antwort_code": "A01" }),
            ),
            (
                "QUOTES",
                serde_json::json!({ "pid": 15_003, "reason": "x" }),
            ),
            (
                "IFTSTA",
                serde_json::json!({ "pid": 21_047, "sts_code": "Z48" }),
            ),
            ("INSRPT", serde_json::json!({ "pid": 23_001, "melo": "x" })),
            (
                "INVOIC",
                serde_json::json!({ "nachricht": "UNH", "sender": "a", "receiver": "b" }),
            ),
            (
                "REMADV",
                serde_json::json!({ "pid": 33_001, "waehrung": "EUR" }),
            ),
            (
                "MSCONS",
                serde_json::json!({ "pid": 13_003, "mabis_zp_id": "x" }),
            ),
        ] {
            assert_eq!(
                undeclared_key(message_type, &declared),
                None,
                "{message_type}"
            );
            let mut unread = declared.clone();
            unread["antwort_tree"] = "E_0203".into();
            let refused = undeclared_key(message_type, &unread)
                .unwrap_or_else(|| panic!("{message_type} admitted an unread key"));
            assert!(
                refused.contains("antwort_tree"),
                "{message_type}: {refused}"
            );
        }
        // Nested parts refuse in the same way.
        let mscons = serde_json::json!({
            "pid": 13_003,
            "intervals": [{ "from": "a", "to": "b", "quantity_kwh": "1", "herkunft": "x" }],
        });
        assert!(undeclared_key("MSCONS", &mscons).is_some());
    }
}
