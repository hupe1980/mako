//! BDEW ERC error codes — structured rejection codes for APERAK and CONTRL.
//!
//! BDEW ERC codes appear in:
//! - **APERAK** `ERC` segments: processability errors returned by the receiving
//!   party when it cannot process a message (BGM+313).
//!
//! `CONTRL` carries no `ERC`: it reports a syntax failure in `UCI`/`UCM`
//! DE 0085, which is a different vocabulary and not this module's.
//!
//! This module provides a validated [`ErcCode`] newtype, a catalogue of
//! standard code string constants in [`codes`], and [`ErcAction`] — a
//! machine-readable recommended automated response for each code.
//! Domain crates `match` on the ERC code to drive typed ERP automation
//! instead of freeform text parsing.
//!
//! # Separation of concerns
//!
//! | Layer | Responsibility |
//! |---|---|
//! | `edi-energy` | Wire-format parsing; raw `String` from ERC segment |
//! | `mako-engine::erc` | Validated type; constants; role-agnostic [`ErcAction`] recommendation |
//! | Domain crates | Process-specific `match` on [`ErcCode`] → domain decision |
//! | `makod` | [`ErcCode`] in outbox payload → `makoerc` CloudEvents extension |
//!
//! # Regulatory sources
//!
//! - APERAK MIG 2.1i / 2.2 — `SG4 ERC` C901 DE 9321, the code table
//! - APERAK AHB 1.0 / 1.1 — which codes each Anwendungsfall admits, and the
//!   Bedingungen that oblige the `SG5 FTX+Z02` Ortsangabe
//! - Allgemeine Festlegungen V6.1d (01.04.2026) — §4 rejection handling
//!
//! # Example
//!
//! ```rust
//! use mako_engine::erc::{ErcCode, ErcAction, codes, recommended_action};
//!
//! let code = ErcCode::new(codes::Z39);
//! assert!(matches!(
//!     recommended_action(&code),
//!     ErcAction::RetryWithCorrection { field: "message" }
//! ));
//! ```

use serde::{Deserialize, Serialize};

// ── ErcCode ───────────────────────────────────────────────────────────────────

/// A BDEW `ERC` DE 9321 error code from an inbound APERAK.
///
/// Wraps an arbitrary string.  Use [`codes`] for known BDEW constants.
/// Use [`ErcCode::new`] for codes parsed from inbound EDIFACT that may not
/// be in the known set (e.g. proprietary NB codes).
///
/// Implements `Serialize`/`Deserialize` as a transparent JSON string so it
/// passes through CloudEvents payloads unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErcCode(Box<str>);

impl ErcCode {
    /// Wrap an arbitrary string as an ERC code.
    ///
    /// No validation is applied — malformed codes from counterparties are
    /// accepted for forensic purposes and matched via `==` or
    /// [`recommended_action`].
    pub fn new(code: impl Into<Box<str>>) -> Self {
        Self(code.into())
    }

    /// Return the code string (e.g. `"Z29"`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ErcCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ErcCode {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ErcCode {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// ── ErcAction ─────────────────────────────────────────────────────────────────

/// Recommended automated response for a received ERC rejection code.
///
/// This is **advice**, not a hard rule.  The ERP decides whether to follow
/// it based on local policy, retry budget, and operator escalation settings.
///
/// Source: BDEW APERAK AHB 1.0; CONTRL AHB 1.0; Allgemeine Festlegungen V6.1d §4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErcAction {
    /// Correct the named field and re-submit the process.
    RetryWithCorrection {
        /// Short identifier of the field to correct
        /// (e.g. `"malo_id"`, `"address"`, `"process_date"`).
        field: &'static str,
    },
    /// Escalate to an operator for manual investigation.
    EscalateToOperator {
        /// Brief reason string for the operator notification.
        reason: &'static str,
    },
    /// Abort the process — the counterparty has definitively rejected it.
    AbortProcess,
    /// Wait for a conflicting in-flight process to finish, then retry.
    WaitAndRetry {
        /// Human-readable description of the blocking condition.
        reason: &'static str,
    },
}

// ── Standard BDEW ERC code string constants ───────────────────────────────────

/// The `ERC` DE 9321 „Anwendungsfehler, Code" values, verbatim from the
/// APERAK MIG 2.1i code table. There is no second vocabulary: `CONTRL`
/// reports syntax failures in `UCI`/`UCM` DE 0085, not in an `ERC`, and a
/// code outside this list is refused by the receiving Marktpartner's own
/// Prüfschablone.
///
/// Which of them a given Anwendungsfall admits is narrower still and is
/// decided by the profile, not here — APERAK AHB 1.0 admits 27 for 29001
/// and none for 29002 (an Anerkennungsmeldung opens no `SG4`).
///
/// These are `&'static str` values so they can be used directly inside
/// `serde_json::json!` macro expressions:
///
/// ```rust
/// use mako_engine::erc::codes;
///
/// let payload = serde_json::json!({ "error_code": codes::Z29 });
/// assert_eq!(payload["error_code"], "Z29");
/// ```
///
/// Use [`ErcCode::new(codes::Z29)`][ErcCode::new] when a rich typed value is
/// needed (e.g. for storing in workflow state or `ErpEventType::AperakRejected`).
pub mod codes {

    /// ID unbekannt.
    pub const Z10: &str = "Z10";

    /// Objekt im IT-System nicht gefunden.
    pub const Z14: &str = "Z14";

    /// Objekt im IT-System nicht eindeutig.
    pub const Z15: &str = "Z15";

    /// Objekt nicht mehr im Netzgebiet.
    pub const Z16: &str = "Z16";

    /// Absender ist zum angegebenen Zeitintervall / Zeitpunkt dem Objekt nicht
    /// zugeordnet.
    pub const Z17: &str = "Z17";

    /// Empfänger ist zum angegebenen Zeitintervall / Zeitpunkt dem Objekt nicht
    /// zugeordnet.
    pub const Z18: &str = "Z18";

    /// Gerätenummer zum angegebenen Zeitintervall / Zeitpunkt an der
    /// Messlokation nicht bekannt.
    pub const Z19: &str = "Z19";

    /// OBIS-Kennzahl zum angegebenen Zeitintervall / Zeitpunkt am Objekt nicht
    /// bekannt.
    pub const Z20: &str = "Z20";

    /// Geschäftsvorfallinterne Referenzierung fehlerhaft.
    pub const Z21: &str = "Z21";

    /// Zuordnungs-Tupel unbekannt.
    pub const Z24: &str = "Z24";

    /// Absender ist zum angegebenen Zeitintervall / Zeitpunkt dem durch das
    /// Zuordnungs-Tupel identifizierten Objekt nicht zugeordnet.
    pub const Z25: &str = "Z25";

    /// Empfänger ist zum angegebenen Zeitintervall / Zeitpunkt dem durch das
    /// Zuordnungs-Tupel identifizierten Objekt nicht zugeordnet.
    pub const Z26: &str = "Z26";

    /// Vorkomma-Stellenzahl des Zählwertes ist zu lang.
    pub const Z27: &str = "Z27";

    /// Erforderliche Angabe für diesen Anwendungsfall fehlt.
    ///
    /// The code for a message that does not satisfy its own Prüfschablone —
    /// what `edi_energy` reports as an `AHB-…-MISSING` finding. Obliges an
    /// `SG5 FTX+Z02` Ortsangabe; see
    /// [`super::requires_ortsangabe`].
    pub const Z29: &str = "Z29";

    /// Zeitreihe unvollständig.
    pub const Z30: &str = "Z30";

    /// Geschäftsvorfall wird vom Empfänger zurückgewiesen.
    ///
    /// The business rejection: the message is well-formed and the receiver
    /// declines it anyway.
    pub const Z31: &str = "Z31";

    /// Referenziertes Geschäftsvorfall-Tupel nicht vorhanden.
    pub const Z33: &str = "Z33";

    /// Zeitintervall negativ oder Null.
    pub const Z34: &str = "Z34";

    /// Format nicht eingehalten. Obliges an Ortsangabe.
    pub const Z35: &str = "Z35";

    /// Geschäftsvorfall darf vom Sender nicht gesendet werden.
    pub const Z37: &str = "Z37";

    /// Anzahl der übermittelten Codes überschreitet Paketdefinition. Obliges an
    /// Ortsangabe.
    pub const Z38: &str = "Z38";

    /// Code nicht aus erlaubtem Wertebereich. Obliges an Ortsangabe.
    pub const Z39: &str = "Z39";

    /// Segment- bzw. Segmentgruppenwiederholbarkeit überschritten. Obliges an
    /// Ortsangabe.
    pub const Z40: &str = "Z40";

    /// Zeitangabe unplausibel. Obliges an Ortsangabe.
    pub const Z41: &str = "Z41";

    /// Konfigurations-ID zum angegebenen Zeitintervall / Zeitpunkt nicht
    /// bekannt.
    pub const Z42: &str = "Z42";

    /// Geschäftsvorfall für Objekt mit der Eigenschaft nicht erlaubt.
    pub const Z43: &str = "Z43";

    /// Eigenschaft des Objekts weicht von der im Geschäftsvorfall codierten
    /// Eigenschaft ab.
    pub const Z44: &str = "Z44";

    /// Every code above, for exhaustiveness checks.
    pub const ALL: [&str; 27] = [
        Z10, Z14, Z15, Z16, Z17, Z18, Z19, Z20, Z21, Z24, Z25, Z26, Z27, Z29, Z30, Z31, Z33, Z34,
        Z35, Z37, Z38, Z39, Z40, Z41, Z42, Z43, Z44,
    ];
}

/// Whether an ERC code obliges the `SG5 FTX+Z02` Ortsangabe des AHB-Fehlers.
///
/// Re-exported from `edi_energy` so a workflow can decide without depending on
/// the wire crate; the list itself is an APERAK AHB fact and lives there.
#[must_use]
pub fn requires_ortsangabe(code: &str) -> bool {
    matches!(
        code,
        codes::Z29 | codes::Z35 | codes::Z38 | codes::Z39 | codes::Z40 | codes::Z41
    )
}

// ── recommended_action ────────────────────────────────────────────────────────

/// Return the recommended automated ERP action for a received ERC code.
///
/// The codes are DE 9321's; an unlisted or proprietary one defaults to
/// [`ErcAction::EscalateToOperator`] so nothing is silently swallowed.
///
/// This is **advice**. What the sender can actually do follows from what the
/// code says is wrong: a code naming a field it can correct is a retry, a code
/// saying the receiver declines is not, and a code about an ID or an assignment
/// it believes to be right is a question for an operator.
///
/// # Example
///
/// ```rust
/// use mako_engine::erc::{ErcCode, ErcAction, codes, recommended_action};
///
/// let code = ErcCode::new(codes::Z31);
/// assert_eq!(recommended_action(&code), ErcAction::AbortProcess);
/// ```
#[must_use]
pub fn recommended_action(code: &ErcCode) -> ErcAction {
    match code.as_str() {
        // The message itself is wrong and the sender can correct it: the code
        // names the segment, the format or the value that failed.
        codes::Z29 | codes::Z35 | codes::Z38 | codes::Z39 | codes::Z40 => {
            ErcAction::RetryWithCorrection { field: "message" }
        }
        codes::Z21 => ErcAction::RetryWithCorrection {
            field: "vorgangsnummer",
        },
        codes::Z27 => ErcAction::RetryWithCorrection { field: "zaehlwert" },
        codes::Z34 | codes::Z41 => ErcAction::RetryWithCorrection {
            field: "zeitintervall",
        },
        codes::Z30 => ErcAction::RetryWithCorrection { field: "zeitreihe" },
        codes::Z19 => ErcAction::RetryWithCorrection {
            field: "geraetenummer",
        },
        codes::Z20 => ErcAction::RetryWithCorrection { field: "obis" },

        // The receiver has decided. Nothing the sender resends changes it.
        codes::Z31 | codes::Z37 | codes::Z43 => ErcAction::AbortProcess,

        // The referenced Vorgang is not there yet — a correlation the receiver
        // may still be processing.
        codes::Z33 => ErcAction::WaitAndRetry {
            reason: "referenced Geschäftsvorfall-Tupel not present at the receiver yet",
        },

        // An identity or an assignment the two sides disagree about: master
        // data, not message content, so a person has to look.
        codes::Z10 | codes::Z14 | codes::Z15 => ErcAction::EscalateToOperator {
            reason: "the receiver does not know this ID — check the Marktpartner master data",
        },
        codes::Z16 => ErcAction::EscalateToOperator {
            reason: "object has left the Netzgebiet — the receiving NB is no longer responsible",
        },
        codes::Z17 | codes::Z18 | codes::Z24 | codes::Z25 | codes::Z26 => {
            ErcAction::EscalateToOperator {
                reason: "party is not assigned to the object at that time — check the Zuordnung",
            }
        }
        codes::Z42 => ErcAction::EscalateToOperator {
            reason: "Konfigurations-ID unknown at the receiver",
        },
        codes::Z44 => ErcAction::EscalateToOperator {
            reason: "the object's property differs from the one the Geschäftsvorfall codes",
        },

        _ => ErcAction::EscalateToOperator {
            reason: "unknown ERC code — manual review required",
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erc_code_roundtrips_json() {
        let code = ErcCode::new(codes::Z29);
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, r#""Z29""#);
        let back: ErcCode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, code);
    }

    #[test]
    fn erc_code_display_matches_inner() {
        let code = ErcCode::new(codes::Z39);
        assert_eq!(code.to_string(), "Z39");
        assert_eq!(code.as_str(), "Z39");
    }

    #[test]
    fn erc_code_from_str() {
        let code = ErcCode::from(codes::Z43);
        assert_eq!(code.as_str(), codes::Z43);
    }

    #[test]
    fn erc_code_as_ref() {
        let code = ErcCode::new(codes::Z31);
        let s: &str = code.as_ref();
        assert_eq!(s, "Z31");
    }

    #[test]
    fn a_message_level_defect_is_a_retry() {
        for c in [codes::Z29, codes::Z35, codes::Z38, codes::Z39, codes::Z40] {
            assert!(
                matches!(
                    recommended_action(&ErcCode::new(c)),
                    ErcAction::RetryWithCorrection { field: "message" }
                ),
                "{c}",
            );
        }
    }

    #[test]
    fn a_receiver_decision_is_not_retried() {
        for c in [codes::Z31, codes::Z37, codes::Z43] {
            assert_eq!(
                recommended_action(&ErcCode::new(c)),
                ErcAction::AbortProcess,
                "{c}",
            );
        }
    }

    #[test]
    fn an_unknown_correlation_waits() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::Z33)),
            ErcAction::WaitAndRetry { .. }
        ));
    }

    #[test]
    fn recommended_action_unknown_escalates() {
        assert!(matches!(
            recommended_action(&ErcCode::new("X99")),
            ErcAction::EscalateToOperator { .. }
        ));
    }

    /// A constant added without a `recommended_action` arm falls through to the
    /// „unknown ERC code" escalation, which reads as a deliberate decision and
    /// is not one.
    #[test]
    fn every_code_has_its_own_recommendation() {
        for c in codes::ALL {
            let action = recommended_action(&ErcCode::new(c));
            assert!(
                !matches!(
                    action,
                    ErcAction::EscalateToOperator {
                        reason: "unknown ERC code — manual review required"
                    }
                ),
                "{c} falls through to the catch-all",
            );
        }
    }

    /// The six codes the APERAK AHB conditions [5] and [9]–[13] name, and only
    /// those. `edi_energy`'s builder emits the `SG5 FTX+Z02` for exactly this
    /// set, so the two lists must not drift.
    #[test]
    fn the_ortsangabe_codes_match_the_ahb_conditions() {
        let obliged: Vec<&str> = codes::ALL
            .into_iter()
            .filter(|c| requires_ortsangabe(c))
            .collect();
        assert_eq!(obliged, ["Z29", "Z35", "Z38", "Z39", "Z40", "Z41"]);
    }

    #[test]
    fn erc_code_in_json_macro() {
        // Ensure codes::* can be used directly in serde_json::json! macros
        // (the primary use case in domain workflow outbox payloads).
        let payload = serde_json::json!({
            "error_code": codes::Z29,
            "reason": "test",
        });
        assert_eq!(payload["error_code"], "Z29");
    }
}
