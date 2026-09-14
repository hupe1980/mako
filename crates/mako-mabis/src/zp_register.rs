//! Which MaBiS-Zählpunkte are currently activated, folded from the lifecycle
//! events.
//!
//! [`zp_lifecycle`](crate::zp_lifecycle) models one Vorgang per process: an
//! Aktivierung and the Deaktivierung that later ends it are two streams that
//! never meet. Nothing therefore answers „which MaBiS-ZP are active right now",
//! and that question has a deadline attached to it.
//!
//! # Why it has a deadline
//!
//! BK6-23-241 Tenorziffer 5 repeals MaBiS Kap. 17.2 with the end of
//! **30.09.2026**, and the tägliche Ausfallarbeitsüberführungszeitreihe is not
//! republished as the Anlage zur BilAReM.
//! [`ZpSerie::endet_am`](crate::zp_lifecycle::ZpSerie::endet_am) carries that
//! date and `ReceiveAnfrage` already refuses to *open* a MaBiS-ZP into a
//! Abrechnungszeitraum past it. A MaBiS-ZP activated **before** the date is the
//! other half: nothing repeals it, it simply stops having a process behind it,
//! and the only symptom is a Summenzeitreihe that never arrives.
//!
//! # What this does and deliberately does not do
//!
//! It **reports**. [`ZpRegister::auf_beendeter_serie`] names the MaBiS-ZP whose
//! series has ended, so an operator can decide. It does not send a
//! Deaktivierung: the repealing Beschluss is not in the mirror, no source in
//! hand says the NB owes one, and a Deaktivierung nobody asked for is a market
//! message sent on a guess.

use std::collections::HashMap;

use mako_engine::{envelope::EventEnvelope, projection::Projection};

use crate::zp_lifecycle::{ZpLifecycleEvent, ZpSerie, ZpVorgang};

/// Every [`ZpLifecycleEvent::event_type`] starts with this.
///
/// Asserted against the enum itself in `the_prefix_covers_every_event_type`, so
/// renaming a variant's wire type cannot quietly empty the register.
const EVENT_TYPE_PREFIX: &str = "MabisZp";

// ── ZpAktivierung ─────────────────────────────────────────────────────────────

/// One MaBiS-Zählpunkt as the fold currently sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZpAktivierung {
    /// The MaBiS-Zählpunkt.
    pub mabis_zp_id: String,
    /// Which series it feeds.
    pub serie: ZpSerie,
    /// Abrechnungszeitraum the last confirmed Vorgang named, as transmitted.
    pub billing_period: String,
    /// `true` while the last confirmed Vorgang was an Aktivierung.
    pub aktiv: bool,
}

/// What one stream's Anfrage named, held until its outcome is known.
#[derive(Debug, Clone)]
struct Anfrage {
    mabis_zp_id: String,
    serie: ZpSerie,
    vorgang: ZpVorgang,
    billing_period: String,
}

// ── ZpRegister ────────────────────────────────────────────────────────────────

/// Read model over every `mabis-zp-lifecycle` stream.
///
/// # How a Vorgang is counted
///
/// Only a **confirmed** Vorgang moves the register. An Anfrage that was
/// refused, or one still waiting for its Antwort, leaves the previous state
/// standing — the MaBiS-ZP is activated when the answering party says so, not
/// when someone asked.
///
/// Three events confirm, and all three are needed:
///
/// - `AntwortGesendet { bestaetigt: true }` — this participant ran the Prüfung
///   and said yes to an inbound Anfrage.
/// - `AntwortErhalten { bestaetigt: true }` — an Anfrage this participant sent
///   was confirmed by the answering party.
/// - `Erfasst` — the family defines no Antwort at all, so the Anfrage standing
///   recorded *is* the outcome. Leaving this out would under-count exactly the
///   families that never answer.
///
/// None of the three carries the Zählpunkt: `mabis_zp_id`, `serie` and
/// `vorgang` live on the `AnfrageErhalten`/`AnfrageGesendet` that opened the
/// stream. The fold therefore remembers the stream's subject and applies the
/// confirmation to it, which is sound because one stream is one Vorgang.
#[derive(Debug, Default)]
pub struct ZpRegister {
    /// What each stream's Anfrage was about, until its outcome is known.
    pending: HashMap<String, Anfrage>,
    /// Current state per MaBiS-Zählpunkt, with the sequence that set it.
    zp: HashMap<String, (ZpAktivierung, u64)>,
    /// Sequence number of the last event applied.
    last_seq: u64,
    /// MaBiS-ZP events that did not decode.
    unlesbar: usize,
}

impl ZpRegister {
    /// Every MaBiS-Zählpunkt the fold has seen, in Zählpunkt order.
    #[must_use]
    pub fn alle(&self) -> Vec<&ZpAktivierung> {
        let mut out: Vec<&ZpAktivierung> = self.zp.values().map(|(a, _)| a).collect();
        out.sort_by(|a, b| a.mabis_zp_id.cmp(&b.mabis_zp_id));
        out
    }

    /// The MaBiS-Zählpunkte still activated for a series that has ended by
    /// `am`.
    ///
    /// Empty for every series a Festlegung has not repealed, which is all of
    /// them but the tägliche AAÜZ.
    #[must_use]
    pub fn auf_beendeter_serie(&self, am: time::Date) -> Vec<&ZpAktivierung> {
        self.alle()
            .into_iter()
            .filter(|a| a.aktiv && !a.serie.gilt_am(am))
            .collect()
    }

    /// How many MaBiS-Zählpunkte are activated right now.
    #[must_use]
    pub fn aktive_anzahl(&self) -> usize {
        self.zp.values().filter(|(a, _)| a.aktiv).count()
    }

    /// MaBiS-ZP events this fold could not decode.
    ///
    /// Non-zero means the register is incomplete — every such event is a
    /// Vorgang whose outcome is missing, so an activation may be reported as
    /// absent. The caller logs it; this crate has no logger of its own.
    #[must_use]
    pub fn unlesbar(&self) -> usize {
        self.unlesbar
    }

    /// Apply a confirmed outcome to the stream's subject.
    fn bestaetigen(&mut self, stream: &str, seq: u64) {
        let Some(a) = self.pending.get(stream).cloned() else {
            return;
        };
        let Anfrage {
            mabis_zp_id: zp_id,
            serie,
            vorgang,
            billing_period,
        } = a;
        let entry = self.zp.entry(zp_id.clone());
        let aktiv = vorgang == ZpVorgang::Aktivierung;
        match entry {
            std::collections::hash_map::Entry::Occupied(mut o) => {
                // A replay can present streams in any order; the later event
                // wins, never the last one fed.
                if o.get().1 > seq {
                    return;
                }
                let (a, s) = o.get_mut();
                a.serie = serie;
                a.aktiv = aktiv;
                a.billing_period = billing_period;
                *s = seq;
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((
                    ZpAktivierung {
                        mabis_zp_id: zp_id,
                        serie,
                        billing_period,
                        aktiv,
                    },
                    seq,
                ));
            }
        }
    }
}

impl Projection for ZpRegister {
    fn name(&self) -> &'static str {
        "MabisZpRegister"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        // Every workflow's events reach this fold — process streams carry no
        // per-domain prefix — so route on `event_type` rather than on whether
        // the payload happens to deserialize. A foreign event whose shape
        // coincided with a variant would otherwise be folded in as one.
        if !envelope.event_type.starts_with(EVENT_TYPE_PREFIX) {
            return;
        }
        let Ok(event) = envelope.decode::<ZpLifecycleEvent>() else {
            // A MaBiS-ZP event that will not decode makes the register
            // under-report, and under-reporting is the direction that stays
            // quiet. This crate carries no logger, so count it and let the
            // caller say so — see `ZpRegister::unlesbar`.
            self.unlesbar += 1;
            return;
        };
        let stream = envelope.stream_id.as_str();
        match event {
            ZpLifecycleEvent::AnfrageErhalten {
                vorgang,
                serie,
                mabis_zp_id,
                billing_period,
                ..
            } => {
                self.pending.insert(
                    stream.to_owned(),
                    Anfrage {
                        mabis_zp_id,
                        serie,
                        vorgang,
                        billing_period: billing_period.as_str().to_owned(),
                    },
                );
            }
            ZpLifecycleEvent::AnfrageGesendet {
                vorgang,
                serie,
                mabis_zp_id,
                billing_period,
                ..
            } => {
                self.pending.insert(
                    stream.to_owned(),
                    Anfrage {
                        mabis_zp_id: mabis_zp_id.to_string(),
                        serie,
                        vorgang,
                        billing_period: billing_period.as_str().to_owned(),
                    },
                );
            }
            ZpLifecycleEvent::AntwortGesendet { bestaetigt, .. }
            | ZpLifecycleEvent::AntwortErhalten { bestaetigt, .. } => {
                if bestaetigt {
                    self.bestaetigen(stream, envelope.sequence_number);
                }
            }
            ZpLifecycleEvent::Erfasst { .. } => {
                self.bestaetigen(stream, envelope.sequence_number);
            }
            ZpLifecycleEvent::WeiterleitungGesendet { .. }
            | ZpLifecycleEvent::ValidationFailed { .. } => {}
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        (self.last_seq != 0).then_some(self.last_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::workflow::EventPayload as _;

    /// The prefix the router keys on covers every event the enum emits.
    ///
    /// `handle_event` sees every workflow's events — process streams carry no
    /// per-domain prefix — so this string is the whole filter. A renamed wire
    /// type would empty the register silently.
    #[test]
    fn the_prefix_covers_every_event_type() {
        let all = [
            ZpLifecycleEvent::Erfasst {
                message_ref: mako_engine::types::MessageRef::new("R"),
            },
            ZpLifecycleEvent::ValidationFailed {
                reason: "r".to_owned(),
            },
            ZpLifecycleEvent::AntwortGesendet {
                antwort_pid: mako_engine::types::Pruefidentifikator::new(55_198)
                    .expect("valid PID"),
                ebd: "E_0071".to_owned(),
                bestaetigt: true,
                grund: None,
            },
            ZpLifecycleEvent::WeiterleitungGesendet {
                weiterleitung_pid: mako_engine::types::Pruefidentifikator::new(55_062)
                    .expect("valid PID"),
                empfaenger: mako_engine::types::MarktpartnerCode::new("9900123456789"),
            },
        ];
        for e in &all {
            assert!(
                e.event_type().starts_with(EVENT_TYPE_PREFIX),
                "{} does not start with {EVENT_TYPE_PREFIX} — the register would drop it",
                e.event_type()
            );
        }
    }
}
