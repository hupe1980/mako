//! (Feature-gated on `reqote`: the fixtures below are REQOTE interchanges, and
//! `just test-features` builds permutations where that message type is off. The
//! §2.13 rule itself is message-type agnostic — see
//! `parse::check_interchange_party_identity`.)
#![cfg(feature = "reqote")]

//! UNB and NAD must name the same parties — BDEW Allgemeine Festlegungen §2.13.
//!
//! > "Die im UNB- und NAD-Segment für den Absender / Empfänger verwendeten
//! > MP-ID sind identisch."
//! >
//! > — Allgemeine Festlegungen V6.1d §2.13
//!
//! The rule applies to every EDI@Energy message, and the reason it is enforced
//! at parse time is an authorisation one: AS4 authenticates the *envelope*
//! sender, while mako's business layer (consent gates, partner lookup, role
//! resolution) reads `NAD+MS`. If the two may disagree, an authenticated
//! partner can attribute a message to a different market participant.

const SENDER: &str = "9900555000005";
const RECEIVER: &str = "9900357000004";
const IMPOSTOR: &str = "9900111000002";

fn interchange(unb_sender: &str, nad_sender: &str) -> Vec<u8> {
    format!(
        "UNB+UNOC:3+{unb_sender}:500+{RECEIVER}:500+260804:1045+REF1'\
UNH+MSG1+REQOTE:D:10A:UN:1.3c'BGM+311+35003'DTM+137:20260804:102'\
RFF+Z13:35003'NAD+MS+{nad_sender}::293'NAD+MR+{RECEIVER}::293'\
UNT+7+MSG1'UNZ+1+REF1'"
    )
    .into_bytes()
}

#[test]
fn a_consistent_interchange_parses() {
    let wire = interchange(SENDER, SENDER);
    edi_energy::parse(&wire).expect("UNB and NAD agree — must parse");
}

/// The defect this check exists to close.
#[test]
fn a_sender_claiming_another_party_in_nad_is_rejected() {
    // The envelope — and therefore the AS4-authenticated identity — is SENDER,
    // but the message body claims to come from IMPOSTOR.
    let wire = interchange(SENDER, IMPOSTOR);
    let err = edi_energy::parse(&wire).expect_err("a party mismatch must not parse");

    match err {
        edi_energy::Error::InterchangePartyMismatch {
            qualifier,
            nad_qualifier,
            ref unb_id,
            ref nad_id,
            message_index,
        } => {
            assert_eq!(qualifier, "DE0004");
            assert_eq!(nad_qualifier, "MS");
            assert_eq!(unb_id, SENDER);
            assert_eq!(nad_id, IMPOSTOR);
            assert_eq!(message_index, 0);
        }
        other => panic!("expected InterchangePartyMismatch, got {other}"),
    }

    // The message must name the rule, so an operator reading a rejected
    // interchange knows why.
    let rendered = edi_energy::parse(&wire).unwrap_err().to_string();
    assert!(rendered.contains("§2.13"), "{rendered}");
}

/// A mismatched receiver is the same defect in the other direction.
#[test]
fn a_receiver_mismatch_is_rejected_too() {
    let wire = format!(
        "UNB+UNOC:3+{SENDER}:500+{RECEIVER}:500+260804:1045+REF1'\
UNH+MSG1+REQOTE:D:10A:UN:1.3c'BGM+311+35003'DTM+137:20260804:102'\
RFF+Z13:35003'NAD+MS+{SENDER}::293'NAD+MR+{IMPOSTOR}::293'\
UNT+7+MSG1'UNZ+1+REF1'"
    )
    .into_bytes();
    let err = edi_energy::parse(&wire).expect_err("a receiver mismatch must not parse");
    assert!(
        matches!(
            err,
            edi_energy::Error::InterchangePartyMismatch {
                nad_qualifier: "MR",
                ..
            }
        ),
        "expected an MR mismatch, got {err}"
    );
}

/// An absent NAD is not a mismatch — some profiles omit a party, and whether
/// that is legal is an AHB question, not an envelope one.
#[test]
fn a_message_without_a_sender_nad_is_not_a_mismatch() {
    let wire = format!(
        "UNB+UNOC:3+{SENDER}:500+{RECEIVER}:500+260804:1045+REF1'\
UNH+MSG1+REQOTE:D:10A:UN:1.3c'BGM+311+35003'DTM+137:20260804:102'\
RFF+Z13:35003'NAD+MR+{RECEIVER}::293'\
UNT+6+MSG1'UNZ+1+REF1'"
    )
    .into_bytes();
    edi_energy::parse(&wire).expect("a missing NAD+MS is an AHB concern, not an envelope one");
}

/// The UNB qualifier must survive parsing.
///
/// S002 is `[0004 identification, 0007 qualifier, 0008 reverse routing]`, so the
/// qualifier is the **second** component; the third is the reverse-routing
/// address, which BDEW interchanges omit — reading it yields an empty string for
/// every real message.
///
/// The qualifier is not decoration: it says which authority issued the MP-ID,
/// and `500` (BDEW) against a GS1 GLN is a Syntaxfehler the counterparty
/// rejects with a CONTRL.
#[test]
fn the_unb_qualifier_is_read_from_the_right_component() {
    let wire = interchange(SENDER, SENDER);
    let parsed = edi_energy::Platform::with_all_profiles()
        .parse_interchange_full(&wire)
        .expect("a consistent interchange parses");
    assert_eq!(parsed.header.sender_qualifier.as_ref(), "500");
    assert_eq!(parsed.header.receiver_qualifier.as_ref(), "500");
    assert_eq!(parsed.header.sender_id.as_ref(), SENDER);
}

/// A GLN sender carries qualifier `14`, and the parser must not normalise it
/// to the BDEW code — the two identify different issuing authorities.
#[test]
fn a_gln_sender_keeps_its_own_qualifier() {
    let wire = format!(
        "UNB+UNOC:3+4012345000023:14+{RECEIVER}:500+260804:1045+REF1'\
UNH+MSG1+REQOTE:D:10A:UN:1.3c'BGM+311+35003'DTM+137:20260804:102'\
RFF+Z13:35003'NAD+MS+4012345000023::9'NAD+MR+{RECEIVER}::293'\
UNT+7+MSG1'UNZ+1+REF1'"
    )
    .into_bytes();
    let parsed = edi_energy::Platform::with_all_profiles()
        .parse_interchange_full(&wire)
        .expect("parses");
    assert_eq!(parsed.header.sender_qualifier.as_ref(), "14");
    assert_eq!(parsed.header.receiver_qualifier.as_ref(), "500");
}
