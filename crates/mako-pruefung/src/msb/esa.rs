//! The three trees an MSB runs when it serves an **Energieserviceanbieter**
//! (WiM Strom Teil 2, Kapitel 4).
//!
//! | Function | Inbound | Answering PIDs | EBD | Frist |
//! |---|---|---|---|---|
//! | [`pruefe_bestellung`] | ORDERS 17007 | 19011 / 19012 | `E_0256` | 2 WT |
//! | [`pruefe_stornierung`] | ORDCHG 39002 | 19013 / 19014 | `E_0257` | 2 WT |
//! | [`pruefe_beendigung`] | ORDERS 17008 | 19011 / 19012 | `E_0254` | 2 WT |
//!
//! # Why these belong here and not in the workflow
//!
//! ORDRSP AHB 1.1b §4.15 makes `SG2 AJT` **Muss** on all four answer PIDs:
//! DE 4465 carries the Prüfschritt code and DE 1082 the tree it belongs to.
//! Conditions `[17]` and `[18]` then require the code to sit in that tree's
//! **Zustimmungs-** resp. **Ablehnungs-Cluster** — so the cluster, not an
//! `accept: bool` the caller passes alongside, is what decides whether the
//! answer rides 19011 or 19012. That is the same rule the NB and LF trees
//! obey, and the reason this crate exists.
//!
//! It also matters that the code is meaningless without its tree. `A01` is
//!
//! - „Die Bindungsfrist des Angebots ist abgelaufen" in `E_0256`,
//! - „Die Bestellung des ESA wurde durch den MSB nicht bestätigt" in `E_0257`,
//! - „Es handelte sich bei der Bestellung um eine einmalige Übermittlung" in
//!   `E_0254`,
//!
//! and all three ride ORDRSP answer PIDs that overlap: 19011/19012 answer both
//! the Bestellung (`E_0256`) and the Beendigung (`E_0254`). The `IMD+7081`
//! Abonnement on the answer is what tells those two apart on the wire, and
//! [`ebd_fuer_antwort`] is that mapping.
//!
//! The two termination paths are not interchangeable: `E_0254` refuses a
//! Beendigung of a one-shot order (`A01`) or one dated before the Abo starts
//! (`A02`), and `E_0257` refuses a Stornierung of a started delivery with
//! **different codes** per Abo mode (`A02` Abo, `A03` one-shot).
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2b, WiM Strom Teil 2 Kap. 4.1–4.3
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 8.25–8.26
//! - ORDRSP AHB 1.1b §4.15, ORDERS AHB 1.1b §4.15, ORDCHG AHB 1.1 §3.2

use time::{Date, OffsetDateTime};

use crate::antwort::RejectReason;
use crate::codes::{EBD_ESA_BEENDIGUNG, EBD_ESA_BESTELLUNG, EBD_ESA_STORNIERUNG, lookup};

use super::types::MsbEntscheidung;

/// Whether the order is a running series or a single transmission.
///
/// Mirrors `IMD+7081` on the wire (`Z01` Start Abo / `Z03` ohne Abo) without
/// depending on `mako-wim`: this crate stays a leaf with no domain deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bestellart {
    /// `Z01` — turnusmäßige/regelmäßige Übermittlung.
    Abo,
    /// `Z03` — einmalige Übermittlung.
    Einmalig,
}

/// The EBD an ORDRSP 19011/19012 must name, given the `IMD+7081` it carries.
///
/// ORDRSP AHB 1.1b §4.15 conditions `[21]`–`[23]`: `Z01`/`Z03` → `E_0256`
/// (the answer to a Bestellung), `Z02` → `E_0254` (the answer to a Beendigung).
/// The Storno answers 19013/19014 always cite `E_0257` and have no `IMD`.
#[must_use]
pub const fn ebd_fuer_antwort(imd_7081: &str) -> Option<&'static str> {
    match imd_7081.as_bytes() {
        b"Z01" | b"Z03" => Some(EBD_ESA_BESTELLUNG),
        b"Z02" => Some(EBD_ESA_BEENDIGUNG),
        _ => None,
    }
}

// ── E_0256 — Bestellung prüfen ────────────────────────────────────────────────

/// What the MSB knows when an ESA Bestellung (ORDERS 17007) arrives.
///
/// Most fields are yes/no because most Prüfschritte are: `E_0256` asks eleven
/// questions and eight of them have a boolean answer the MSB's own records
/// give. Collapsing them into flag words would make the call sites unreadable
/// and lose the one-field-per-Prüfschritt correspondence an auditor needs.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq)]
pub struct EsaBestellung {
    /// End of the Bindungsfrist the MSB stated in its own Angebot.
    pub bindungsfrist: OffsetDateTime,
    /// When the Bestellung was received (the ÜT from the AS4-Zustellquittung).
    pub eingegangen_am: OffsetDateTime,
    /// Whether the MSB honours an order that arrived after its Bindungsfrist
    /// (Prüfschritt 2 — a commercial decision, not a rule).
    pub akzeptiert_nach_bindungsfrist: bool,
    /// Abo or one-shot (`IMD+7081`).
    pub art: Bestellart,
    /// Whether the MSB offers this Messprodukt in the requested mode
    /// (Prüfschritte 4/5).
    pub messprodukt_lieferbar: bool,
    /// Whether the ESA-Rahmenvertrag is still in force for the requested period
    /// (Prüfschritt 6).
    pub vertrag_gueltig: bool,
    /// Whether the MSB is assigned to the location for that period
    /// (Prüfschritt 7).
    pub zugeordnet: bool,
    /// Whether the datenschutzrechtliche Einwilligung is still valid
    /// (Prüfschritt 8). `None` when the MSB holds no record — self-assertion,
    /// which BNetzA *Mitteilung Nr. 3* forbids rejecting on.
    pub einwilligung_gueltig: Option<bool>,
    /// Whether the installed Gerätetechnik can produce the values
    /// (Prüfschritt 9).
    pub geraetetechnik_geeignet: bool,
    /// `true` for a Marktlokation-, Tranchen- or Netzlokations-level order,
    /// which needs the Prüfschritt-11 bundle check (`false` for a Messlokation).
    pub gebuendelte_ebene: bool,
    /// For a bundled level: whether the MSB operates **every** underlying
    /// Messlokation (Prüfschritt 11). `None` when the bundle is not known —
    /// answering that from an incomplete registry would be a guess.
    pub msb_aller_messlokationen: Option<bool>,
}

/// Walk `E_0256` — the MSB's answer to an ESA Bestellung von Werten.
///
/// # Panics
///
/// Only if the `E_0256` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_bestellung(b: &EsaBestellung) -> MsbEntscheidung {
    let tree = EBD_ESA_BESTELLUNG;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0256");

    // 1/2 — Bindungsfrist abgelaufen, und nimmt der MSB sie trotzdem an?
    if b.eingegangen_am > b.bindungsfrist && !b.akzeptiert_nach_bindungsfrist {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A01"),
            2,
            format!(
                "Die Bindungsfrist des Angebots endete am {}, die Bestellung ging am {} ein",
                b.bindungsfrist, b.eingegangen_am
            ),
        ));
    }

    // 3/4/5 — bietet der MSB das Messprodukt in der bestellten Betriebsart an?
    if !b.messprodukt_lieferbar {
        let (c, schritt, was) = match b.art {
            Bestellart::Abo => ("A04", 4, "als Abo"),
            Bestellart::Einmalig => ("A05", 5, "als einmalige Übermittlung"),
        };
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code(c),
            schritt,
            format!("Der MSB bietet das gewünschte Messprodukt nicht {was} an"),
        ));
    }

    // 6 — vertragliche Grundlage (ESA-Rahmenvertrag).
    if !b.vertrag_gueltig {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A06"),
            6,
            "Die vertragliche Grundlage zwischen dem MSB und dem ESA ist zum Zeitraum der \
             Messwertermittlung nicht mehr gültig",
        ));
    }

    // 7 — Zuordnung des MSB zur Lokation im angefragten Zeitraum.
    if !b.zugeordnet {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A07"),
            7,
            "Der MSB ist der Lokation für den im Angebot spezifizierten Zeitraum nicht zugeordnet",
        ));
    }

    // 8 — Einwilligung. An *explicitly* invalid consent refuses; an absent
    // record does not, because the MSB holds only the ESA's Zusicherung and
    // BNetzA Mitteilung Nr. 3 (07.02.2024) forbids rejecting on consent form.
    if b.einwilligung_gueltig == Some(false) {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A08"),
            8,
            "Der Anschlussnutzer hat gegenüber dem ESA seine Einwilligung widerrufen oder ihre \
             Gültigkeit ist abgelaufen",
        ));
    }

    // 9 — Gerätetechnik.
    if !b.geraetetechnik_geeignet {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A09"),
            9,
            "Die Gerätetechnik misst die angeforderten Messwerte nicht",
        ));
    }

    // 10 — Messlokations-Bestellung: fertig.
    if !b.gebuendelte_ebene {
        return MsbEntscheidung::accept(tree, code("A11"));
    }

    // 11 — gebündelte Ebene: der MSB muss auch MSB *aller* Messlokationen sein.
    match b.msb_aller_messlokationen {
        Some(true) => MsbEntscheidung::accept(tree, code("A11")),
        Some(false) => MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A10"),
            11,
            "Der MSB der Marktlokation / Netzlokation ist nicht zeitgleich der allen \
             Messlokationen zugeordnete MSB",
        )),
        None => MsbEntscheidung::Escalate {
            reason: "E_0256 Prüfschritt 11: ob der MSB an allen Messlokationen des \
                     Lokationsbündels den Messstellenbetrieb führt, ist nicht bekannt"
                .to_owned(),
        },
    }
}

// ── E_0257 — Stornierung prüfen ───────────────────────────────────────────────

/// What the MSB knows when an ESA Stornierung (ORDCHG 39002) arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EsaStornierung {
    /// Whether the MSB confirmed the underlying Bestellung (Prüfschritt 1).
    pub bestellung_bestaetigt: bool,
    /// Abo or one-shot (Prüfschritt 2).
    pub art: Bestellart,
    /// Whether any values have gone out under the order yet
    /// (Prüfschritte 3/4 — the same fact, two codes).
    pub uebermittlung_begonnen: bool,
}

/// Walk `E_0257` — the MSB's answer to an ESA Stornierung einer Bestellung.
///
/// # Panics
///
/// Only if the `E_0257` Codeliste is missing a code this function names.
#[must_use]
pub fn pruefe_stornierung(s: &EsaStornierung) -> MsbEntscheidung {
    let tree = EBD_ESA_STORNIERUNG;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0257");

    // 1 — nur eine bestätigte Bestellung ist stornierbar.
    if !s.bestellung_bestaetigt {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A01"),
            1,
            "Die Bestellung des ESA wurde durch den MSB nicht bestätigt",
        ));
    }

    // 2/3/4 — dieselbe Tatsache, zwei Codes: die Betriebsart entscheidet.
    if s.uebermittlung_begonnen {
        let (c, schritt, detail) = match s.art {
            Bestellart::Abo => (
                "A02",
                3,
                "Mit der Übermittlung von Werten aus dem Abo wurde bereits begonnen",
            ),
            Bestellart::Einmalig => (
                "A03",
                4,
                "Die einmalige Übermittlung der Werte ist bereits erfolgt",
            ),
        };
        return MsbEntscheidung::Reject(RejectReason::new(tree, code(c), schritt, detail));
    }

    MsbEntscheidung::accept(tree, code("A04"))
}

// ── E_0254 — Beendigung prüfen ────────────────────────────────────────────────

/// What the MSB knows when an ESA Abbestellung (ORDERS 17008) arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EsaBeendigung {
    /// Abo or one-shot (Prüfschritt 1).
    pub art: Bestellart,
    /// Requested end date (`DTM+203` on the 17008).
    pub beendigung_zum: Date,
    /// Start of the running Abo (Prüfschritt 2).
    pub abo_beginn: Date,
    /// Date the delivery was already ended, if it was (Prüfschritt 3).
    pub bereits_beendet_zum: Option<Date>,
    /// Date the most recent values covered, if any went out (Prüfschritt 4).
    pub juengste_lieferung: Option<Date>,
}

/// Walk `E_0254` — the MSB's answer to an ESA Abbestellung von Werten.
///
/// # Panics
///
/// Only if the `E_0254` Codeliste is missing a code this function names.
#[must_use]
pub fn pruefe_beendigung(b: &EsaBeendigung) -> MsbEntscheidung {
    let tree = EBD_ESA_BEENDIGUNG;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0254");

    // 1 — eine einmalige Übermittlung wird storniert, nicht abbestellt.
    if b.art == Bestellart::Einmalig {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A01"),
            1,
            "Es handelte sich bei der Bestellung um eine einmalige Übermittlung — sie ist zu \
             stornieren (ORDCHG 39002), nicht abzubestellen",
        ));
    }

    // 2 — ein Ende vor dem Abo-Beginn ist ebenfalls eine Stornierung.
    if b.beendigung_zum <= b.abo_beginn {
        return MsbEntscheidung::Reject(
            RejectReason::new(
                tree,
                code("A02"),
                2,
                format!(
                    "Das gewünschte Beendigungsdatum {} liegt nicht nach dem Abo-Beginn {} — \
                     die Bestellung ist zu stornieren",
                    b.beendigung_zum, b.abo_beginn
                ),
            )
            .mit_termin(b.abo_beginn),
        );
    }

    // 3 — bereits beendet.
    if let Some(beendet) = b.bereits_beendet_zum
        && beendet <= b.beendigung_zum
    {
        return MsbEntscheidung::Reject(
            RejectReason::new(
                tree,
                code("A03"),
                3,
                format!("Die Übermittlung wurde bereits zum {beendet} beendet"),
            )
            .mit_termin(beendet),
        );
    }

    // 4 — es wurden schon Daten nach dem gewünschten Ende geliefert.
    if let Some(juengste) = b.juengste_lieferung
        && juengste > b.beendigung_zum
    {
        return MsbEntscheidung::Reject(
            RejectReason::new(
                tree,
                code("A04"),
                4,
                format!(
                    "Es wurden bereits Daten bis zum {juengste} übermittelt, also nach dem \
                     gewünschten Beendigungsdatum {}",
                    b.beendigung_zum
                ),
            )
            .mit_termin(juengste),
        );
    }

    MsbEntscheidung::accept(tree, code("A05"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    fn bestellung() -> EsaBestellung {
        EsaBestellung {
            bindungsfrist: datetime!(2026-03-31 23:59 UTC),
            eingegangen_am: datetime!(2026-03-02 10:00 UTC),
            akzeptiert_nach_bindungsfrist: false,
            art: Bestellart::Abo,
            messprodukt_lieferbar: true,
            vertrag_gueltig: true,
            zugeordnet: true,
            einwilligung_gueltig: Some(true),
            geraetetechnik_geeignet: true,
            gebuendelte_ebene: false,
            msb_aller_messlokationen: None,
        }
    }

    #[test]
    fn a_clean_bestellung_is_accepted_with_a11() {
        let d = pruefe_bestellung(&bestellung());
        assert_eq!(d.antwortcode(), Some("A11"));
        assert!(matches!(d, MsbEntscheidung::Accept(_)));
    }

    /// Prüfschritt 2 is a commercial decision, not a rule: the MSB may honour
    /// a late order.
    #[test]
    fn a_late_bestellung_is_refused_unless_the_msb_accepts_it() {
        let late = EsaBestellung {
            eingegangen_am: datetime!(2026-04-02 10:00 UTC),
            ..bestellung()
        };
        assert_eq!(pruefe_bestellung(&late).antwortcode(), Some("A01"));
        let honoured = EsaBestellung {
            akzeptiert_nach_bindungsfrist: true,
            ..late
        };
        assert_eq!(pruefe_bestellung(&honoured).antwortcode(), Some("A11"));
    }

    /// Prüfschritte 4 and 5 refuse the same fact with different codes; the
    /// Abo mode is what selects between them.
    #[test]
    fn an_unavailable_messprodukt_refuses_per_betriebsart() {
        let abo = EsaBestellung {
            messprodukt_lieferbar: false,
            ..bestellung()
        };
        assert_eq!(pruefe_bestellung(&abo).antwortcode(), Some("A04"));
        let einmalig = EsaBestellung {
            art: Bestellart::Einmalig,
            ..abo
        };
        assert_eq!(pruefe_bestellung(&einmalig).antwortcode(), Some("A05"));
    }

    /// BNetzA Mitteilung Nr. 3: absence of a consent record is the ESA's
    /// self-assertion and must not be refused. Only an explicitly invalid one
    /// reaches `A08`.
    #[test]
    fn an_unknown_consent_does_not_refuse_but_an_invalid_one_does() {
        let unknown = EsaBestellung {
            einwilligung_gueltig: None,
            ..bestellung()
        };
        assert_eq!(pruefe_bestellung(&unknown).antwortcode(), Some("A11"));
        let invalid = EsaBestellung {
            einwilligung_gueltig: Some(false),
            ..bestellung()
        };
        assert_eq!(pruefe_bestellung(&invalid).antwortcode(), Some("A08"));
    }

    /// Prüfschritt 11 exists because a MaLo/Tranche/NeLo order presupposes one
    /// MSB across the whole bundle (UC 4.1.1 Vorbedingung). An unknown bundle
    /// escalates instead of guessing.
    #[test]
    fn a_bundled_order_checks_every_messlokation_and_escalates_when_unknown() {
        let bundled = EsaBestellung {
            gebuendelte_ebene: true,
            ..bestellung()
        };
        assert!(matches!(
            pruefe_bestellung(&bundled),
            MsbEntscheidung::Escalate { .. }
        ));
        let split = EsaBestellung {
            msb_aller_messlokationen: Some(false),
            ..bundled
        };
        assert_eq!(pruefe_bestellung(&split).antwortcode(), Some("A10"));
        let whole = EsaBestellung {
            msb_aller_messlokationen: Some(true),
            ..bundled
        };
        assert_eq!(pruefe_bestellung(&whole).antwortcode(), Some("A11"));
    }

    #[test]
    fn stornierung_needs_a_confirmed_order_and_an_unstarted_delivery() {
        let ok = EsaStornierung {
            bestellung_bestaetigt: true,
            art: Bestellart::Abo,
            uebermittlung_begonnen: false,
        };
        assert_eq!(pruefe_stornierung(&ok).antwortcode(), Some("A04"));
        assert_eq!(
            pruefe_stornierung(&EsaStornierung {
                bestellung_bestaetigt: false,
                ..ok
            })
            .antwortcode(),
            Some("A01")
        );
    }

    /// „Delivery has begun" is one fact with two codes — `A02` for a running
    /// Abo, `A03` for a one-shot that already went out.
    #[test]
    fn a_started_delivery_refuses_the_storno_per_betriebsart() {
        let abo = EsaStornierung {
            bestellung_bestaetigt: true,
            art: Bestellart::Abo,
            uebermittlung_begonnen: true,
        };
        assert_eq!(pruefe_stornierung(&abo).antwortcode(), Some("A02"));
        let einmalig = EsaStornierung {
            art: Bestellart::Einmalig,
            ..abo
        };
        assert_eq!(pruefe_stornierung(&einmalig).antwortcode(), Some("A03"));
    }

    fn beendigung() -> EsaBeendigung {
        EsaBeendigung {
            art: Bestellart::Abo,
            beendigung_zum: date!(2026 - 06 - 01),
            abo_beginn: date!(2026 - 03 - 01),
            bereits_beendet_zum: None,
            juengste_lieferung: Some(date!(2026 - 05 - 20)),
        }
    }

    #[test]
    fn a_clean_beendigung_is_confirmed_with_a05() {
        assert_eq!(pruefe_beendigung(&beendigung()).antwortcode(), Some("A05"));
    }

    /// The two termination paths are disjoint: a one-shot is stornierbar, and
    /// so is an Abo whose end date precedes its own start.
    #[test]
    fn the_termination_paths_do_not_overlap() {
        let einmalig = EsaBeendigung {
            art: Bestellart::Einmalig,
            ..beendigung()
        };
        assert_eq!(pruefe_beendigung(&einmalig).antwortcode(), Some("A01"));
        let vor_beginn = EsaBeendigung {
            beendigung_zum: date!(2026 - 02 - 01),
            ..beendigung()
        };
        assert_eq!(pruefe_beendigung(&vor_beginn).antwortcode(), Some("A02"));
    }

    #[test]
    fn ending_behind_already_delivered_values_is_refused() {
        let d = pruefe_beendigung(&EsaBeendigung {
            beendigung_zum: date!(2026 - 05 - 01),
            ..beendigung()
        });
        assert_eq!(d.antwortcode(), Some("A04"));
    }

    #[test]
    fn a_second_beendigung_is_refused_as_already_ended() {
        let d = pruefe_beendigung(&EsaBeendigung {
            bereits_beendet_zum: Some(date!(2026 - 05 - 01)),
            ..beendigung()
        });
        assert_eq!(d.antwortcode(), Some("A03"));
    }

    /// 19011/19012 answer two different processes; `IMD+7081` is what says
    /// which tree the `AJT` code came from.
    #[test]
    fn the_imd_selects_the_tree_for_the_shared_answer_pids() {
        assert_eq!(ebd_fuer_antwort("Z01"), Some(EBD_ESA_BESTELLUNG));
        assert_eq!(ebd_fuer_antwort("Z03"), Some(EBD_ESA_BESTELLUNG));
        assert_eq!(ebd_fuer_antwort("Z02"), Some(EBD_ESA_BEENDIGUNG));
        assert_eq!(ebd_fuer_antwort("Z99"), None);
    }

    /// Every code the three walks name must be published by its tree, and on
    /// the cluster the walk puts it on.
    #[test]
    fn every_named_code_is_published_on_the_right_cluster() {
        use crate::codes::Cluster;
        for (tree, code, cluster) in [
            (EBD_ESA_BESTELLUNG, "A11", Cluster::Zustimmung),
            (EBD_ESA_BESTELLUNG, "A01", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A04", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A05", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A06", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A07", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A08", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A09", Cluster::Ablehnung),
            (EBD_ESA_BESTELLUNG, "A10", Cluster::Ablehnung),
            (EBD_ESA_STORNIERUNG, "A04", Cluster::Zustimmung),
            (EBD_ESA_STORNIERUNG, "A01", Cluster::Ablehnung),
            (EBD_ESA_STORNIERUNG, "A02", Cluster::Ablehnung),
            (EBD_ESA_STORNIERUNG, "A03", Cluster::Ablehnung),
            (EBD_ESA_BEENDIGUNG, "A05", Cluster::Zustimmung),
            (EBD_ESA_BEENDIGUNG, "A01", Cluster::Ablehnung),
            (EBD_ESA_BEENDIGUNG, "A02", Cluster::Ablehnung),
            (EBD_ESA_BEENDIGUNG, "A03", Cluster::Ablehnung),
            (EBD_ESA_BEENDIGUNG, "A04", Cluster::Ablehnung),
        ] {
            let c = lookup(tree, code).unwrap_or_else(|| panic!("{tree}/{code} missing"));
            assert_eq!(c.cluster, cluster, "{tree}/{code}");
        }
    }

    /// The same spelling means three different things across the three trees —
    /// which is why a code may never be resolved without naming its tree.
    #[test]
    fn a01_means_three_different_things() {
        let meanings: Vec<_> = [EBD_ESA_BESTELLUNG, EBD_ESA_STORNIERUNG, EBD_ESA_BEENDIGUNG]
            .iter()
            .map(|t| lookup(t, "A01").unwrap().bedeutung)
            .collect();
        assert_eq!(meanings.len(), 3);
        assert_eq!(
            meanings
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "A01 must mean something different in each tree: {meanings:?}"
        );
    }
}
