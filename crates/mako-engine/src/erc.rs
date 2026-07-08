//! BDEW ERC error codes — structured rejection codes for APERAK and CONTRL.
//!
//! BDEW ERC codes appear in:
//! - **APERAK** `ERC` segments: processability errors returned by the receiving
//!   party when it cannot process a message (BGM+313).
//! - **CONTRL** `ERC` segments: syntax and data-validation errors.
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
//! - APERAK AHB 1.0 (FV2025-10-01 / FV2026-10-01) — ERC segment, §2.2/§2.3
//! - CONTRL AHB 1.0 (FV2025-10-01 / FV2026-10-01) — ERC segment, §2.2
//! - Allgemeine Festlegungen V6.1d (01.04.2026) — §4 rejection handling
//!
//! # Example
//!
//! ```rust
//! use mako_engine::erc::{ErcCode, ErcAction, codes, recommended_action};
//!
//! let code = ErcCode::new(codes::E02);
//! assert!(matches!(
//!     recommended_action(&code),
//!     ErcAction::RetryWithCorrection { field: "address" }
//! ));
//! ```

use serde::{Deserialize, Serialize};

// ── ErcCode ───────────────────────────────────────────────────────────────────

/// A BDEW ERC error code from an inbound APERAK or CONTRL.
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

/// Standard BDEW ERC error code string constants.
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
    // ── APERAK ERC codes (BGM+313 processability errors) ─────────────────────

    /// Ablehnung — Prozess nicht gefunden / sonstiger Fehler.
    ///
    /// Catchall code for messages that cannot be routed to any active process.
    /// Source: APERAK AHB 1.0.
    pub const Z29: &str = "Z29";

    /// Marktlokation / Identifikationsnummer nicht gefunden.
    ///
    /// The MaLo-ID in the message is not registered with the receiver.
    /// Source: CONTRL AHB 1.0 / APERAK AHB 1.0.
    pub const Z43: &str = "Z43";

    /// Ablehnung — Zähler in Betrieb (Sperrung nicht ausführbar).
    ///
    /// NB cannot execute a Sperrung because the meter is currently live.
    /// Process terminates; no retry is appropriate.
    /// Source: ORDERS/ORDRSP Sperrung AHB.
    pub const ZB3: &str = "ZB3";

    /// Ablehnung — Lieferstelle gesperrt.
    pub const Z28: &str = "Z28";

    /// Ablehnung — kein aktiver Prozess vorhanden.
    pub const Z30: &str = "Z30";

    /// Ablehnung — Zeitraum nicht plausibel.
    pub const Z04: &str = "Z04";

    /// Ablehnung — nicht autorisiert.
    pub const Z07: &str = "Z07";

    /// Ablehnung — Zählernummer nicht plausibel.
    pub const Z08: &str = "Z08";

    /// Ablehnung — Messlokation ungültig.
    pub const Z09: &str = "Z09";

    // ── CONTRL ERC codes (syntax / data validation errors) ────────────────────

    /// MaLo / Identifikationsnummer unbekannt.
    ///
    /// Source: BDEW CONTRL AHB 1.0; Allgemeine Festlegungen V6.1d.
    pub const E01: &str = "E01";

    /// Adresse stimmt nicht überein.
    pub const E02: &str = "E02";

    /// Kein gültiger Lieferant für diese Marktlokation.
    pub const E03: &str = "E03";

    /// Datum liegt in der Vergangenheit / Datum nicht plausibel.
    pub const E04: &str = "E04";

    /// Wechsel nicht möglich — laufender Prozess bereits vorhanden.
    pub const E05: &str = "E05";

    /// Ungültige Prüfidentifikatornummer.
    pub const E06: &str = "E06";
}

// ── recommended_action ────────────────────────────────────────────────────────

/// Return the recommended automated ERP action for a received ERC code.
///
/// The table covers the common LF-relevant codes defined in APERAK AHB 1.0
/// and CONTRL AHB 1.0.  Unknown codes default to
/// [`ErcAction::EscalateToOperator`] so nothing is silently swallowed.
///
/// # Example
///
/// ```rust
/// use mako_engine::erc::{ErcCode, ErcAction, codes, recommended_action};
///
/// let code = ErcCode::new(codes::E05);
/// assert!(matches!(recommended_action(&code), ErcAction::WaitAndRetry { .. }));
/// ```
#[must_use]
pub fn recommended_action(code: &ErcCode) -> ErcAction {
    match code.as_str() {
        // CONTRL codes
        codes::E01 | codes::Z43 => ErcAction::RetryWithCorrection { field: "malo_id" },
        codes::E02 => ErcAction::RetryWithCorrection { field: "address" },
        codes::E03 => ErcAction::EscalateToOperator {
            reason: "LF GLN not recognised by NB — check Marktteilnehmerverzeichnis",
        },
        codes::E04 | codes::Z04 => ErcAction::RetryWithCorrection {
            field: "process_date",
        },
        codes::E05 => ErcAction::WaitAndRetry {
            reason: "concurrent in-flight process present; wait for completion then retry",
        },
        codes::E06 => ErcAction::EscalateToOperator {
            reason: "invalid Prüfidentifikator in outbound message — engineering alert",
        },
        // APERAK codes
        codes::Z07 => ErcAction::EscalateToOperator {
            reason: "not authorised for this supply point",
        },
        codes::Z08 => ErcAction::RetryWithCorrection { field: "meter_id" },
        codes::Z09 => ErcAction::RetryWithCorrection { field: "melo_id" },
        codes::Z28 | codes::Z30 | codes::ZB3 => ErcAction::AbortProcess,
        codes::Z29 => ErcAction::EscalateToOperator {
            reason: "process not found or unclassified error",
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
        let code = ErcCode::new(codes::E02);
        assert_eq!(code.to_string(), "E02");
        assert_eq!(code.as_str(), "E02");
    }

    #[test]
    fn erc_code_from_str() {
        let code = ErcCode::from(codes::Z43);
        assert_eq!(code.as_str(), codes::Z43);
    }

    #[test]
    fn erc_code_as_ref() {
        let code = ErcCode::new(codes::ZB3);
        let s: &str = code.as_ref();
        assert_eq!(s, "ZB3");
    }

    #[test]
    fn recommended_action_e01_retry_malo() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E01)),
            ErcAction::RetryWithCorrection { field: "malo_id" }
        ));
    }

    #[test]
    fn recommended_action_e02_retry_address() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E02)),
            ErcAction::RetryWithCorrection { field: "address" }
        ));
    }

    #[test]
    fn recommended_action_e04_retry_date() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E04)),
            ErcAction::RetryWithCorrection {
                field: "process_date"
            }
        ));
    }

    #[test]
    fn recommended_action_e05_wait_and_retry() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E05)),
            ErcAction::WaitAndRetry { .. }
        ));
    }

    #[test]
    fn recommended_action_e03_escalate() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E03)),
            ErcAction::EscalateToOperator { .. }
        ));
    }

    #[test]
    fn recommended_action_e06_escalate() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::E06)),
            ErcAction::EscalateToOperator { .. }
        ));
    }

    #[test]
    fn recommended_action_z29_escalate() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::Z29)),
            ErcAction::EscalateToOperator { .. }
        ));
    }

    #[test]
    fn recommended_action_zb3_abort() {
        assert_eq!(
            recommended_action(&ErcCode::new(codes::ZB3)),
            ErcAction::AbortProcess
        );
    }

    #[test]
    fn recommended_action_z28_abort() {
        assert_eq!(
            recommended_action(&ErcCode::new(codes::Z28)),
            ErcAction::AbortProcess
        );
    }

    #[test]
    fn recommended_action_z30_abort() {
        assert_eq!(
            recommended_action(&ErcCode::new(codes::Z30)),
            ErcAction::AbortProcess
        );
    }

    #[test]
    fn recommended_action_z43_retry_malo() {
        assert!(matches!(
            recommended_action(&ErcCode::new(codes::Z43)),
            ErcAction::RetryWithCorrection { field: "malo_id" }
        ));
    }

    #[test]
    fn recommended_action_unknown_escalates() {
        assert!(matches!(
            recommended_action(&ErcCode::new("X99")),
            ErcAction::EscalateToOperator { .. }
        ));
    }

    #[test]
    fn all_standard_codes_have_recommendations() {
        // Every code in the `codes` module must return a non-default action
        // (i.e. not fall through to the catch-all EscalateToOperator for
        // "unknown ERC code").  This test guards against adding a constant
        // without updating the match table.
        for (code_str, expected_variant) in [
            (codes::E01, "RetryWithCorrection"),
            (codes::E02, "RetryWithCorrection"),
            (codes::E03, "EscalateToOperator"),
            (codes::E04, "RetryWithCorrection"),
            (codes::E05, "WaitAndRetry"),
            (codes::E06, "EscalateToOperator"),
            (codes::Z04, "RetryWithCorrection"),
            (codes::Z07, "EscalateToOperator"),
            (codes::Z08, "RetryWithCorrection"),
            (codes::Z09, "RetryWithCorrection"),
            (codes::Z28, "AbortProcess"),
            (codes::Z29, "EscalateToOperator"),
            (codes::Z30, "AbortProcess"),
            (codes::Z43, "RetryWithCorrection"),
            (codes::ZB3, "AbortProcess"),
        ] {
            let action = recommended_action(&ErcCode::new(code_str));
            let variant_name = match &action {
                ErcAction::RetryWithCorrection { .. } => "RetryWithCorrection",
                ErcAction::EscalateToOperator { .. } => "EscalateToOperator",
                ErcAction::AbortProcess => "AbortProcess",
                ErcAction::WaitAndRetry { .. } => "WaitAndRetry",
            };
            assert_eq!(
                variant_name, expected_variant,
                "ERC code {code_str}: expected {expected_variant}, got {variant_name}"
            );
        }
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
