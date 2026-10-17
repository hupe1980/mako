//! `[policy.dsr]` — the data protection officer's readings for a data-subject
//! request, which the DSGVO leaves to the controller.
//!
//! Two questions the article text does not settle:
//!
//! - **Which categories are portable.** Art. 20 Abs. 1 DSGVO grants the data
//!   „die sie einem Verantwortlichen bereitgestellt hat", where the processing
//!   rests on consent or on a contract under Art. 6 Abs. 1 lit. b and „mithilfe
//!   automatisierter Verfahren erfolgt". Whether a category counts as
//!   *bereitgestellt* is the DPO's reading, per category: `portabel.<kategorie>`.
//! - **Which processes are automated decisions.** Art. 15 Abs. 1 lit. h DSGVO
//!   requires the Auskunft to state „das Bestehen einer automatisierten
//!   Entscheidungsfindung einschließlich Profiling gemäß Artikel 22 Absätze 1
//!   und 4"; whether automatic dunning or a Sperr escalation is one is the DPO's
//!   reading, per process: `automatisiert.<prozess>`.
//!
//! Both default to `true`: stating more than the reading requires cannot breach
//! Art. 15 or Art. 20, stating less can.
//!
//! ```toml
//! [policy.dsr.portabel]
//! zahlungsinformation = false
//!
//! [policy.dsr.automatisiert]
//! mahnwesen = false
//! ```

use std::collections::BTreeMap;

use mako_service::policy::{Policy, PolicyEntry, PolicyTriple};
use serde::{Deserialize, Serialize};

/// A category of personal data `vertragd` holds about a data subject — one per
/// section of the DSGVO export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DsrKategorie {
    /// The Kunde record: Geschäftspartner, numbers, notes.
    Kunde,
    /// The natural person's name, address and contact.
    Person,
    /// Bank details and payment method.
    Zahlungsinformation,
    /// Portal logins.
    Identitaeten,
    /// Supply contracts.
    Vertraege,
    /// Contract components (product, MaLo, dates).
    Komponenten,
}

impl DsrKategorie {
    /// Every category, in export order.
    pub const ALL: [Self; 6] = [
        Self::Kunde,
        Self::Person,
        Self::Zahlungsinformation,
        Self::Identitaeten,
        Self::Vertraege,
        Self::Komponenten,
    ];

    /// The snake-case name used in the config and the export.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kunde => "kunde",
            Self::Person => "person",
            Self::Zahlungsinformation => "zahlungsinformation",
            Self::Identitaeten => "identitaeten",
            Self::Vertraege => "vertraege",
            Self::Komponenten => "komponenten",
        }
    }
}

/// A process whose outcome reaches the data subject without a human decision
/// in the individual case, as far as the DPO reads Art. 22 Abs. 1 DSGVO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DsrProzess {
    /// Automatic dunning runs.
    Mahnwesen,
    /// Escalation of an overdue account to a Sperrung.
    Sperreskalation,
}

impl DsrProzess {
    /// Every process, in statement order.
    pub const ALL: [Self; 2] = [Self::Mahnwesen, Self::Sperreskalation];

    /// The snake-case name used in the config and the export.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mahnwesen => "mahnwesen",
            Self::Sperreskalation => "sperreskalation",
        }
    }
}

mako_service::policy_choice! {
    /// Whether a category is part of the Art. 20 DSGVO document.
    pub enum Portabel {
        key: "dsr.portabel",
        doc: "@/docs/services/vertragd.md#policy-dsr",
        default: tightening(Ja, "more disclosure cannot breach Art. 20; withholding a portable category can"),
        options: {
            /// The category is part of the Art. 20 document.
            Ja = "true" => "Art. 20 Abs. 1 DSGVO: Daten, die die betroffene Person bereitgestellt hat, bei Verarbeitung auf Grundlage eines Vertrags (Art. 6 Abs. 1 lit. b) mithilfe automatisierter Verfahren",
            /// The DPO reads the category as not *bereitgestellt*; it stays in
            /// the Art. 15 Auskunft and leaves the Art. 20 document.
            Nein = "false" => "Art. 20 Abs. 1 DSGVO: nur „bereitgestellte“ Daten sind übertragbar — die Kategorie gilt nach der Lesart des Datenschutzbeauftragten nicht als bereitgestellt",
        }
    }
}

mako_service::policy_choice! {
    /// Whether the Auskunft states a process as an automated decision.
    pub enum Automatisiert {
        key: "dsr.automatisiert",
        doc: "@/docs/services/vertragd.md#policy-dsr",
        default: tightening(Ja, "more disclosure cannot breach Art. 15; omitting an automated decision can"),
        options: {
            /// The Auskunft states the process as an automated decision.
            Ja = "true" => "Art. 15 Abs. 1 lit. h DSGVO i. V. m. Art. 22 Abs. 1 DSGVO: Bestehen einer automatisierten Entscheidungsfindung",
            /// The DPO reads the process as no decision under Art. 22 Abs. 1.
            Nein = "false" => "Art. 22 Abs. 1 DSGVO: keine ausschließlich auf einer automatisierten Verarbeitung beruhende Entscheidung — Lesart des Datenschutzbeauftragten",
        }
    }
}

/// `[policy.dsr]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DsrPolicy {
    /// Per category; a category not listed is portable.
    #[serde(default)]
    pub portabel: BTreeMap<DsrKategorie, Policy<Portabel>>,
    /// Per process; a process not listed is stated.
    #[serde(default)]
    pub automatisiert: BTreeMap<DsrProzess, Policy<Automatisiert>>,
}

impl DsrPolicy {
    /// The reading for one category — the listed value, or the default.
    #[must_use]
    pub fn portabel(&self, k: DsrKategorie) -> Policy<Portabel> {
        self.portabel.get(&k).copied().unwrap_or_default()
    }

    /// The reading for one process — the listed value, or the default.
    #[must_use]
    pub fn automatisiert(&self, p: DsrProzess) -> Policy<Automatisiert> {
        self.automatisiert.get(&p).copied().unwrap_or_default()
    }

    /// One report entry per category and per process, keyed
    /// `dsr.portabel.<kategorie>` / `dsr.automatisiert.<prozess>`.
    #[must_use]
    pub fn entries(&self) -> Vec<PolicyEntry> {
        let mut out = Vec::new();
        for k in DsrKategorie::ALL {
            let mut e = self.portabel(k).entry();
            e.key = format!("dsr.portabel.{}", k.as_str());
            out.push(e);
        }
        for p in DsrProzess::ALL {
            let mut e = self.automatisiert(p).entry();
            e.key = format!("dsr.automatisiert.{}", p.as_str());
            out.push(e);
        }
        out
    }

    /// The Art. 20 section of the export: which categories are portable, and
    /// the reading each was decided under.
    #[must_use]
    pub fn datenuebertragbarkeit(&self) -> serde_json::Value {
        let entscheidungen: Vec<serde_json::Value> = DsrKategorie::ALL
            .iter()
            .map(|&k| {
                let p = self.portabel(k);
                serde_json::json!({
                    "kategorie": k.as_str(),
                    "portabel": p.get() == Portabel::Ja,
                    "policy": keyed(p.triple(), &format!("dsr.portabel.{}", k.as_str())),
                })
            })
            .collect();
        let kategorien: Vec<&str> = DsrKategorie::ALL
            .iter()
            .filter(|&&k| self.portabel(k).get() == Portabel::Ja)
            .map(|k| k.as_str())
            .collect();
        serde_json::json!({
            "rechtsgrundlage": "Art. 20 Abs. 1 DSGVO",
            "kategorien": kategorien,
            "entscheidungen": entscheidungen,
        })
    }

    /// The Art. 15 Abs. 1 lit. h statement: each process, whether it is stated
    /// as an automated decision, and the reading behind it.
    #[must_use]
    pub fn automatisierte_entscheidungen(&self) -> serde_json::Value {
        let prozesse: Vec<serde_json::Value> = DsrProzess::ALL
            .iter()
            .map(|&p| {
                let a = self.automatisiert(p);
                serde_json::json!({
                    "prozess": p.as_str(),
                    "automatisierte_entscheidung": a.get() == Automatisiert::Ja,
                    "policy": keyed(a.triple(), &format!("dsr.automatisiert.{}", p.as_str())),
                })
            })
            .collect();
        serde_json::json!({
            "rechtsgrundlage": "Art. 15 Abs. 1 lit. h DSGVO i. V. m. Art. 22 DSGVO",
            "prozesse": prozesse,
        })
    }
}

fn keyed(mut t: PolicyTriple, key: &str) -> PolicyTriple {
    key.clone_into(&mut t.key);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a `portabel` entry a category is portable: the Art. 20 section
    /// lists it, and its decision names the default reading. A category the
    /// DPO reads as not provided leaves the section and says why.
    #[test]
    fn an_unlisted_category_is_portable_and_a_listed_false_one_is_not() {
        let unset = DsrPolicy::default();
        let doc = unset.datenuebertragbarkeit();
        assert_eq!(doc["kategorien"].as_array().unwrap().len(), 6, "{doc}");
        assert!(
            doc["kategorien"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("zahlungsinformation"))
        );
        assert_eq!(doc["entscheidungen"][2]["policy"]["value"], "true");

        let set: DsrPolicy = serde_json::from_value(serde_json::json!({
            "portabel": { "zahlungsinformation": false }
        }))
        .unwrap();
        let doc = set.datenuebertragbarkeit();
        assert!(
            !doc["kategorien"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("zahlungsinformation")),
            "{doc}"
        );
        let e = &doc["entscheidungen"][2];
        assert_eq!(e["portabel"], false);
        assert_eq!(e["policy"]["key"], "dsr.portabel.zahlungsinformation");
        assert_eq!(e["policy"]["value"], "false");
    }

    #[test]
    fn every_process_is_stated_by_default_and_reported_once() {
        let p = DsrPolicy::default();
        let s = p.automatisierte_entscheidungen();
        assert!(
            s["prozesse"]
                .as_array()
                .unwrap()
                .iter()
                .all(|x| x["automatisierte_entscheidung"] == true)
        );
        let keys: Vec<String> = p.entries().into_iter().map(|e| e.key).collect();
        assert!(keys.contains(&"dsr.automatisiert.mahnwesen".to_owned()));
        assert_eq!(keys.len(), 8);
    }

    #[test]
    fn an_unknown_category_is_refused() {
        let r: Result<DsrPolicy, _> = serde_json::from_value(serde_json::json!({
            "portabel": { "zaehlerstaende": false }
        }));
        assert!(r.is_err());
    }
}
