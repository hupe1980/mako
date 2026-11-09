//! Configuration findings — what a daemon reports about the configuration it
//! was given, as data.
//!
//! [`Daemon::validate`](crate::Daemon::validate) returns every [`Befund`] it
//! finds rather than stopping at the first: a config with two defects reports
//! both, so reaching a running deployment is not one restart per defect. The
//! runner derives the boot refusal from the list — any [`Schwere::Blockiert`]
//! refuses, every [`Schwere::Warnung`] is logged — and serves the same list at
//! `GET /admin/einrichtung` and from `--check-config`, so what blocks a boot
//! and what the operator is shown are one list and cannot disagree.
//!
//! A finding names the setting's path in the configuration file
//! ([`Befund::pfad`]), its operator-policy anchor where the setting is a
//! [`PolicyChoice`] ([`Befund::referenz`]) and the document the rule rests on
//! ([`Befund::quelle`]) — a primary source, never a paraphrase.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::policy::PolicyChoice;

/// Whether a finding refuses the boot.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Schwere {
    /// The daemon refuses to start on it.
    Blockiert,
    /// The daemon starts and warns at every boot.
    Warnung,
}

impl Schwere {
    /// `blockiert` / `warnung`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blockiert => "blockiert",
            Self::Warnung => "warnung",
        }
    }
}

/// Which readiness question of the deployment-wide checklist a finding
/// belongs to.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Bereich {
    /// MP-IDs per Rolle and Sparte and the agency code each implies.
    Identitaeten,
    /// AS4 and API-Webdienste certificates, key material and their expiry.
    Zertifikate,
    /// Bilanzkreise per Sparte.
    Bilanzierung,
    /// Authentication, Cedar policies, the posture of every door.
    Zugriff,
    /// The services and endpoints a daemon depends on.
    Erreichbarkeit,
    /// Profiles and adapters per format version in force.
    Formatabdeckung,
    /// Everything else a configuration states.
    Konfiguration,
}

impl Bereich {
    /// Every area, in checklist order.
    pub const ALLE: [Self; 7] = [
        Self::Identitaeten,
        Self::Zertifikate,
        Self::Bilanzierung,
        Self::Zugriff,
        Self::Erreichbarkeit,
        Self::Formatabdeckung,
        Self::Konfiguration,
    ];

    /// The readiness question, as the checklist asks it.
    #[must_use]
    pub const fn frage(self) -> &'static str {
        match self {
            Self::Identitaeten => {
                "Jede Rolle hat je Sparte genau eine MP-ID mit der Codenummer, die sie verlangt"
            }
            Self::Zertifikate => {
                "Jedes Zertifikat ist lesbar, passt zu seinem Schlüssel und läuft nicht ab"
            }
            Self::Bilanzierung => "Jeder Bilanzkreis ist seiner Sparte eindeutig zugeordnet",
            Self::Zugriff => "Jede Tür ist authentifiziert und autorisiert",
            Self::Erreichbarkeit => "Jeder Dienst und jede Gegenstelle ist erreichbar",
            Self::Formatabdeckung => {
                "Jede gültige Formatversion ist durch Profil und Adapter abgedeckt"
            }
            Self::Konfiguration => "Jede weitere Einstellung ist gültig",
        }
    }
}

/// One finding about a daemon's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Befund {
    /// Whether it refuses the boot.
    pub schwere: Schwere,
    /// The checklist area it belongs to.
    #[serde(default = "konfiguration")]
    pub bereich: Bereich,
    /// What is wrong, in the daemon's own words.
    pub text: String,
    /// The setting's path in the configuration file, dotted as the loader
    /// names it (`lf.bilanzkreise[1].sparte`, `party[0].mp_id`); empty for a
    /// finding about the file as a whole.
    pub pfad: String,
    /// The setting's operator-policy reference entry
    /// (`@/docs/services/<svc>.md#policy-<key>`), for a [`PolicyChoice`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenz: Option<String>,
    /// The document and chapter the rule rests on, where a document fixes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quelle: Option<String>,
}

const fn konfiguration() -> Bereich {
    Bereich::Konfiguration
}

impl Befund {
    fn neu(schwere: Schwere, pfad: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            schwere,
            bereich: Bereich::Konfiguration,
            text: text.into(),
            pfad: pfad.into(),
            referenz: None,
            quelle: None,
        }
    }

    /// A finding that refuses the boot.
    #[must_use]
    pub fn blockiert(pfad: impl Into<String>, text: impl Into<String>) -> Self {
        Self::neu(Schwere::Blockiert, pfad, text)
    }

    /// A finding the daemon starts with and warns about.
    #[must_use]
    pub fn warnung(pfad: impl Into<String>, text: impl Into<String>) -> Self {
        Self::neu(Schwere::Warnung, pfad, text)
    }

    /// A blocking finding from a check that reports an error value — the
    /// error's whole chain becomes the text.
    #[must_use]
    pub fn aus_fehler(pfad: impl Into<String>, fehler: impl fmt::Display) -> Self {
        Self::blockiert(pfad, format!("{fehler:#}"))
    }

    /// The checklist area.
    #[must_use]
    pub fn bereich(mut self, bereich: Bereich) -> Self {
        self.bereich = bereich;
        self
    }

    /// The document and chapter the rule rests on.
    #[must_use]
    pub fn quelle(mut self, quelle: impl Into<String>) -> Self {
        self.quelle = Some(quelle.into());
        self
    }

    /// The reference entry of a free-form setting, `@/docs/….md#anchor`.
    #[must_use]
    pub fn referenz(mut self, referenz: impl Into<String>) -> Self {
        self.referenz = Some(referenz.into());
        self
    }

    /// The operator-policy reference entry of the setting `T`
    /// ([`PolicyChoice::DOC`]) — what a setting's own validation names.
    #[must_use]
    pub fn referenz_auf<T: PolicyChoice>(self) -> Self {
        self.referenz(T::DOC)
    }

    /// Whether it refuses the boot.
    #[must_use]
    pub fn ist_blockiert(&self) -> bool {
        self.schwere == Schwere::Blockiert
    }

    /// A problem the config loader found, as a finding: the file did not load,
    /// so every one blocks.
    #[must_use]
    pub fn aus_config_problem(p: &crate::ConfigProblem) -> Self {
        use crate::ConfigProblem as P;
        let pfad = match p {
            P::Missing(k) | P::Unknown(k) => k.clone(),
            P::Invalid { path, .. } | P::PolicyRequired { path, .. } => path.clone(),
            P::FileNotFound(_) | P::Unreadable(_) | P::Incomplete => String::new(),
        };
        Self::blockiert(pfad, p.to_string())
    }
}

impl fmt::Display for Befund {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pfad.is_empty() {
            write!(f, "{}", self.text)?;
        } else {
            write!(f, "[{}] {}", self.pfad, self.text)?;
        }
        if let Some(q) = &self.quelle {
            write!(f, " (Quelle: {q})")?;
        }
        if let Some(r) = &self.referenz {
            write!(f, " (siehe {r})")?;
        }
        Ok(())
    }
}

/// The blocking findings of a configuration — the error a boot refuses with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verweigerung(pub Vec<Befund>);

impl fmt::Display for Verweigerung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0.len();
        write!(
            f,
            "{n} blocking configuration finding{}:",
            if n == 1 { "" } else { "s" }
        )?;
        for b in &self.0 {
            write!(f, "\n  - {b}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Verweigerung {}

/// Split `befunde` into a refusal (every blocking finding) or the warnings a
/// daemon starts with.
///
/// # Errors
///
/// Every [`Schwere::Blockiert`] finding, when there is one.
pub fn als_ergebnis(befunde: Vec<Befund>) -> Result<Vec<Befund>, Verweigerung> {
    let (blockiert, warnungen): (Vec<_>, Vec<_>) =
        befunde.into_iter().partition(Befund::ist_blockiert);
    if blockiert.is_empty() {
        Ok(warnungen)
    } else {
        Err(Verweigerung(blockiert))
    }
}

/// Log the findings: `error` per blocking one, `warn` per warning.
pub fn log(dienst: &str, befunde: &[Befund]) {
    for b in befunde {
        match b.schwere {
            Schwere::Blockiert => tracing::error!(target: "mako_einrichtung", "{dienst}: {b}"),
            Schwere::Warnung => tracing::warn!(target: "mako_einrichtung", "{dienst}: {b}"),
        }
    }
}

/// The default warning window for a certificate's expiry, in days.
pub const ZERTIFIKAT_WARNFENSTER_TAGE: u32 = 30;

/// A finding for a certificate that has expired (blocking) or expires within
/// `warnfenster_tage` of `jetzt` (a warning), naming its expiry date; `None`
/// when it is valid beyond the window.
#[must_use]
pub fn zertifikat_ablauf(
    pfad: &str,
    was: &str,
    gueltig_bis: time::OffsetDateTime,
    jetzt: time::OffsetDateTime,
    warnfenster_tage: u32,
) -> Option<Befund> {
    let datum = gueltig_bis.date();
    if gueltig_bis <= jetzt {
        return Some(
            Befund::blockiert(pfad, format!("{was} ist am {datum} abgelaufen"))
                .bereich(Bereich::Zertifikate),
        );
    }
    let rest = gueltig_bis - jetzt;
    (rest <= time::Duration::days(i64::from(warnfenster_tage))).then(|| {
        Befund::warnung(
            pfad,
            format!(
                "{was} läuft am {datum} ab, in {} Tagen — innerhalb des Warnfensters von \
                 {warnfenster_tage} Tagen",
                rest.whole_days()
            ),
        )
        .bereich(Bereich::Zertifikate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_every_blocking_finding_and_keeps_the_warnings_apart() {
        let befunde = vec![
            Befund::blockiert("a.b", "erstens"),
            Befund::warnung("c", "nur eine Warnung"),
            Befund::blockiert("d[1]", "zweitens").quelle("Dokument, Kap. 1"),
        ];
        let err = als_ergebnis(befunde.clone()).unwrap_err();
        assert_eq!(err.0.len(), 2);
        let text = err.to_string();
        assert!(text.contains("[a.b] erstens"), "{text}");
        assert!(
            text.contains("[d[1]] zweitens (Quelle: Dokument, Kap. 1)"),
            "{text}"
        );
        assert!(!text.contains("Warnung"), "{text}");
        let ok = als_ergebnis(vec![befunde[1].clone()]).unwrap();
        assert_eq!(ok, vec![befunde[1].clone()]);
    }

    #[test]
    fn a_finding_serialises_with_its_path_reference_and_source() {
        let b = Befund::blockiert("party[0].mp_id", "falsche Codenummer")
            .bereich(Bereich::Identitaeten)
            .quelle("BDEW-AWH Identifikatoren V1.2, Kap. 2.2")
            .referenz("@/docs/services/makod.md#party");
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schwere": "blockiert",
                "bereich": "identitaeten",
                "text": "falsche Codenummer",
                "pfad": "party[0].mp_id",
                "referenz": "@/docs/services/makod.md#party",
                "quelle": "BDEW-AWH Identifikatoren V1.2, Kap. 2.2",
            })
        );
        let back: Befund = serde_json::from_value(json).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn a_certificate_is_red_once_expired_and_amber_inside_the_window() {
        let jetzt = time::macros::datetime!(2026-11-09 12:00 UTC);
        let abgelaufen =
            zertifikat_ablauf("x", "Signatur", jetzt - time::Duration::days(1), jetzt, 30).unwrap();
        assert_eq!(abgelaufen.schwere, Schwere::Blockiert);
        assert!(
            abgelaufen.text.contains("2026-11-08"),
            "{}",
            abgelaufen.text
        );
        let bald = zertifikat_ablauf("x", "Signatur", jetzt + time::Duration::days(10), jetzt, 30)
            .unwrap();
        assert_eq!(bald.schwere, Schwere::Warnung);
        assert_eq!(bald.bereich, Bereich::Zertifikate);
        assert!(bald.text.contains("2026-11-19"), "{}", bald.text);
        assert!(
            zertifikat_ablauf("x", "Signatur", jetzt + time::Duration::days(31), jetzt, 30)
                .is_none()
        );
    }

    #[test]
    fn a_loader_problem_becomes_a_blocking_finding_at_its_path() {
        let b = Befund::aus_config_problem(&crate::ConfigProblem::Invalid {
            path: "lf.bilanzkreise[0]".into(),
            message: "missing field `sparte`".into(),
        });
        assert!(b.ist_blockiert());
        assert_eq!(b.pfad, "lf.bilanzkreise[0]");
        assert!(b.text.contains("sparte"), "{}", b.text);
    }
}
