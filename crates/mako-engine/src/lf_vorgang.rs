//! The `de.mako.process.initiated` contract for an **LF-answered Vorgang**.
//!
//! Nine inbound Prüfidentifikatoren put a supplier in the answering seat, split
//! across two Festlegungen — GPKE Strom (`mako-gpke`) and GeLi Gas
//! (`mako-geli-gas`) — but answered by **one** decision path in `processd`.
//! The workflows differ per Sparte; the facts their notification must carry do
//! not, because [`mako_pruefung`]'s trees branch on the same five `SG4`
//! elements whichever Festlegung the message came from.
//!
//! So the contract lives here rather than in either workflow crate: both depend
//! on `mako-engine` already, and a payload built twice is a payload that drifts.
//! A Gas payload without the Transaktionsgrundergänzung escalates every Gas walk
//! at Prüfschritt 10.
//!
//! [`mako_pruefung`]: https://docs.rs/mako-pruefung
//!
//! # Absent is not null
//!
//! [`LfVorgangsdaten::process_initiated`] omits a field the message did not
//! carry instead of writing `null`. A consumer that tests *presence* — and
//! `DTM+471` is exactly such a test, because its presence **is** the answer to
//! `E_0614` Prüfschritt 60 — reads `Some(Value::Null)` as „present", and every
//! Kündigung then looks like one „zum nächstmöglichen Termin": the branch that
//! may not be refused for Vertragsbindung.

use crate::outbox::PendingOutbox;
use crate::types::{MaLo, MarktpartnerCode, Pruefidentifikator};

/// The `SG4` facts an LF-answered Vorgang carries beyond its Lokations-ID.
///
/// Every LF tree branches on at least one of them, so a
/// `de.mako.process.initiated` that omits them cannot be walked: the
/// Transaktionsgrundergänzung picks the code range, the Transaktionsgrund picks
/// the branch, `DTM+154` starts `E_0624`'s own Frist and `DTM+471` decides
/// whether `E_0614` may refuse for Vertragsbindung at all.
///
/// Kept in one type so the Strom and Gas workflows cannot drift apart on what
/// they publish, and so a new fact is added once.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LfVorgangsdaten {
    /// `SG4 STS+7` DE 9013 element 2 — `Z33`, `ZQ7`, `ZT0`, `E01`, `E03`, …
    pub transaktionsgrund: Option<String>,
    /// `SG4 STS+7` DE 9013 element 3 — `ZW3` / `ZW4` / `ZW5` / `ZAP`.
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG4 IDE+24` DE 7402 — the sender's Vorgangsnummer.
    ///
    /// The answer references it in `SG4 RFF+TN`; it is never reused as the
    /// answer's own `IDE+24`, which the MIG requires to be globally unique.
    pub vorgangsnummer: Option<String>,
    /// `SG4 DTM+154` — ÜT der Lieferanmeldung des LFN, on a 55010.
    ///
    /// `E_0624` Prüfschritt 5 measures its own Frist from it and answers `A43`
    /// when the NB asked too late.
    pub uet_lieferanmeldung: Option<String>,
    /// `SG4 DTM+471` — „Ende zum nächstmöglichen Termin", on a 55016 / 44016.
    ///
    /// Present **instead of** `DTM+93`. `E_0614` Prüfschritt 60 branches on
    /// which of the two arrived, and only the fixed date may be refused for
    /// Vertragsbindung — so this field's *presence* is load-bearing and it is
    /// omitted, never nulled, when the message carried a fixed date.
    pub naechstmoeglicher_termin: Option<String>,
    /// `SG12 NAD+Z09` `C080` — „Kunde des LF", the customer name the request
    /// carries, joined from the composite's up-to-five DE 3036 components.
    ///
    /// `E_0624` Prüfschritt 50 („Ist der Kunde aus der Anfrage zur Beendigung
    /// der Zuordnung identisch mit dem Kunden beim LFA?") is answerable only
    /// from this: the UTILMD AHB marks the segment **Muss** on a 55010 whose
    /// Transaktionsgrundergänzung is `ZW4`/`ZAP` (Bedingung `[279]`), and
    /// Bedingung `[572]` says what it is — „Kundenname aus Anmeldung Lieferant
    /// neu". Without it the whole Ein-/Auszug arm (`A32`/`A33`/`A34`)
    /// escalates, which is a large share of all switches.
    pub kunde_name: Option<String>,
    /// `SG12 NAD+Z09` `C080` DE 3045 — the Namensformat: `Z01` Struktur von
    /// Personennamen, `Z02` Struktur der Firmenbezeichnung.
    ///
    /// It says how to read [`Self::kunde_name`]: five interchangeable
    /// components are a person (Nachname, Vorname, …) under `Z01` and a company
    /// name under `Z02`, and a comparison that ignores the difference matches a
    /// person against a company.
    pub kunde_namensformat: Option<String>,
    /// `SG12 NAD+VY` DE 3039 — the **Neulieferant**'s MP-ID (Bedingung `[567]`).
    ///
    /// The 55010 is the only message that names the LFN to the LFA before the
    /// switch completes; it is what reconciles an Anfrage against a Kündigung
    /// the LFA already answered.
    pub lfn_mp_id: Option<String>,
}

impl LfVorgangsdaten {
    /// The `de.mako.process.initiated` notification for an inbound Vorgang.
    ///
    /// Without it `processd`'s LF module never sees the message: `makod`
    /// delivers a CloudEvent only for an outbox entry, and an APERAK is a
    /// technical acknowledgement, not a business notification.
    ///
    /// `extra` merges Sparte-specific facts that other consumers need — the
    /// Gas Bilanzierungsmethode, Fallgruppe and Gasqualität `marktd` folds into
    /// the Marktlokation. Keys it does not set are left alone, so an `extra`
    /// can never silently drop a fact a tree branches on.
    ///
    /// # Panics
    ///
    /// Never in practice: the panic guards the `json!` literal below, which is
    /// an object by construction.
    #[must_use]
    pub fn process_initiated(
        &self,
        pid: Pruefidentifikator,
        malo_id: &MaLo,
        sender: &MarktpartnerCode,
        receiver: &MarktpartnerCode,
        process_date: &str,
        extra: &serde_json::Value,
    ) -> PendingOutbox {
        let mut payload = serde_json::json!({
            "pid":           pid.as_u32(),
            "malo_id":       malo_id.as_str(),
            "sender":        sender.as_str(),
            "receiver":      receiver.as_str(),
            // The counterparty is the NB on 55007/55010/55607/44007/44010 and
            // the LFN on a 55016/44016; `processd` reads whichever is set.
            "grid_operator": sender.as_str(),
            "process_date":  process_date,
            "termin":        process_date,
        });

        // Absent, not null — see the module docs.
        let obj = payload.as_object_mut().expect("json! built an object");
        let mut set = |key: &str, value: &Option<String>| {
            if let Some(v) = value {
                obj.insert(key.to_owned(), serde_json::Value::String(v.clone()));
            }
        };
        set("transaktionsgrund", &self.transaktionsgrund);
        set(
            "transaktionsgrund_ergaenzung",
            &self.transaktionsgrund_ergaenzung,
        );
        set("vorgangsnummer", &self.vorgangsnummer);
        set("uet_lieferanmeldung", &self.uet_lieferanmeldung);
        set("naechstmoeglicher_termin", &self.naechstmoeglicher_termin);
        set("kunde_name", &self.kunde_name);
        set("kunde_namensformat", &self.kunde_namensformat);
        set("lfn_mp_id", &self.lfn_mp_id);

        if let Some(extra) = extra.as_object() {
            for (k, v) in extra {
                obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }

        PendingOutbox::new("ProcessInitiated", receiver.as_str(), payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox(vorgang: &LfVorgangsdaten, extra: &serde_json::Value) -> serde_json::Value {
        vorgang
            .process_initiated(
                Pruefidentifikator::new(55_016).expect("valid PID"),
                &MaLo::new("51238696012"),
                &MarktpartnerCode::new("9900357000004"),
                &MarktpartnerCode::new("9900000000001"),
                "20260901",
                extra,
            )
            .payload
            .clone()
    }

    /// A fact the message did not carry is **absent**, not `null`. `DTM+471`'s
    /// presence is the answer to `E_0614` Prüfschritt 60, so a `null` there
    /// reads as „Kündigung zum nächstmöglichen Termin" — the branch that may
    /// never be refused for Vertragsbindung.
    #[test]
    fn a_missing_fact_is_absent_rather_than_null() {
        let p = outbox(&LfVorgangsdaten::default(), &serde_json::Value::Null);
        for key in [
            "transaktionsgrund",
            "transaktionsgrund_ergaenzung",
            "vorgangsnummer",
            "uet_lieferanmeldung",
            "naechstmoeglicher_termin",
            "kunde_name",
            "kunde_namensformat",
            "lfn_mp_id",
        ] {
            assert!(p.get(key).is_none(), "{key} must be absent, got {p:#}");
        }
    }

    /// Every fact the trees branch on reaches the payload.
    #[test]
    fn every_branching_fact_is_carried() {
        let p = outbox(
            &LfVorgangsdaten {
                transaktionsgrund: Some("E03".into()),
                transaktionsgrund_ergaenzung: Some("ZW4".into()),
                vorgangsnummer: Some("NNV1234".into()),
                uet_lieferanmeldung: Some("20260820".into()),
                naechstmoeglicher_termin: Some("20261231".into()),
                kunde_name: Some("Mustermann Erika".into()),
                kunde_namensformat: Some("Z01".into()),
                lfn_mp_id: Some("9900357000004".into()),
            },
            &serde_json::Value::Null,
        );
        assert_eq!(p["transaktionsgrund"], "E03");
        assert_eq!(p["transaktionsgrund_ergaenzung"], "ZW4");
        assert_eq!(p["vorgangsnummer"], "NNV1234");
        assert_eq!(p["uet_lieferanmeldung"], "20260820");
        assert_eq!(p["naechstmoeglicher_termin"], "20261231");
        assert_eq!(p["kunde_name"], "Mustermann Erika");
        assert_eq!(p["kunde_namensformat"], "Z01");
        assert_eq!(p["lfn_mp_id"], "9900357000004");
    }

    /// Sparte-specific facts ride along, and cannot displace a fact a tree
    /// branches on.
    #[test]
    fn extras_are_merged_without_overwriting() {
        let p = outbox(
            &LfVorgangsdaten {
                transaktionsgrund: Some("E03".into()),
                ..LfVorgangsdaten::default()
            },
            &serde_json::json!({
                "bilanzierungsmethode": "SLP",
                "gasqualitaet":         "H-Gas",
                "transaktionsgrund":    "SHOULD-NOT-WIN",
            }),
        );
        assert_eq!(p["bilanzierungsmethode"], "SLP");
        assert_eq!(p["gasqualitaet"], "H-Gas");
        assert_eq!(p["transaktionsgrund"], "E03");
    }

    /// The notification is addressed to us — the party that must answer.
    #[test]
    fn the_notification_is_addressed_to_the_answering_party() {
        let ob = LfVorgangsdaten::default().process_initiated(
            Pruefidentifikator::new(55_007).expect("valid PID"),
            &MaLo::new("51238696012"),
            &MarktpartnerCode::new("9900357000004"),
            &MarktpartnerCode::new("9900000000001"),
            "20260901",
            &serde_json::Value::Null,
        );
        assert_eq!(&*ob.recipient, "9900000000001");
        assert_eq!(ob.payload["grid_operator"], "9900357000004");
    }
}
