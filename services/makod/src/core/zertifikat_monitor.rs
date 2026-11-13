//! The expiry of every configured AS4 certificate, re-read while makod runs.
//!
//! The startup check reports a certificate that has expired or expires inside
//! `as4.zertifikat_warnfenster_tage` — but a daemon that runs for months passes
//! that window without another startup. [`ZertifikatMonitor`] re-reads every
//! `[party.as4]` and `[[as4.partner]]` certificate file on a schedule and on
//! each request, so:
//!
//! - `GET /admin/einrichtung` carries the findings as they stand now, in place
//!   of the boot's;
//! - `GET /metrics` exports `makod_certificate_expiry_days{pfad}` — the whole
//!   days left (negative once expired), so an alert fires before a send fails;
//! - a certificate entering the window or expiring is logged once per check.
//!
//! The holder of a certificate makes its successor available at least 10
//! Werktage before it becomes invalid (Regelungen zum Übertragungsweg für AS4
//! 2.6, Kap. 5.3, S. 6); the default window of 30 days covers that with room
//! for the request to the Sub-CA.

use std::sync::{Arc, RwLock};

use mako_service::befund::{self, Befund};
use mako_service::einrichtung::Einrichtung;

use crate::config::{As4Config, PartyConfig};
use crate::einrichtung::{Zertifikat, zertifikate};

/// How often the monitor re-reads the certificate files between requests.
pub const PRUEFINTERVALL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// The configured certificates' expiry, as last read.
#[derive(Debug)]
pub struct ZertifikatMonitor {
    as4: As4Config,
    parties: Vec<PartyConfig>,
    fenster: u32,
    stand: RwLock<Vec<Zertifikat>>,
}

impl ZertifikatMonitor {
    /// A monitor over `as4`'s and `parties`' certificates, read once now.
    #[must_use]
    pub fn new(as4: &As4Config, parties: &[PartyConfig]) -> Arc<Self> {
        let monitor = Arc::new(Self {
            fenster: as4
                .zertifikat_warnfenster_tage
                .unwrap_or(befund::ZERTIFIKAT_WARNFENSTER_TAGE),
            as4: as4.clone(),
            parties: parties.to_vec(),
            stand: RwLock::new(Vec::new()),
        });
        monitor.lies();
        monitor
    }

    /// Re-read every certificate file.
    pub fn lies(&self) -> Vec<Zertifikat> {
        let neu = zertifikate(&self.as4, &self.parties);
        if let Ok(mut stand) = self.stand.write() {
            stand.clone_from(&neu);
        }
        neu
    }

    /// The certificates as last read.
    #[must_use]
    pub fn zertifikate(&self) -> Vec<Zertifikat> {
        self.stand.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// The expiry findings at `jetzt`.
    #[must_use]
    pub fn befunde(&self, jetzt: time::OffsetDateTime) -> Vec<Befund> {
        self.zertifikate()
            .iter()
            .filter_map(|z| {
                befund::zertifikat_ablauf(&z.pfad, &z.was, z.gueltig_bis, jetzt, self.fenster)
            })
            .collect()
    }

    /// `boot` with its certificate-expiry findings replaced by those at
    /// `jetzt` — a certificate that entered the window since the boot is
    /// reported, and one replaced on disk is no longer.
    #[must_use]
    pub fn einrichtung(&self, boot: &Einrichtung, jetzt: time::OffsetDateTime) -> Einrichtung {
        let pfade = self.pfade();
        let mut befunde: Vec<Befund> = boot
            .befunde
            .iter()
            .filter(|b| !pfade.contains(&b.pfad))
            .cloned()
            .collect();
        befunde.extend(self.befunde(jetzt));
        Einrichtung::neu(
            &boot.dienst,
            befunde,
            boot.policy.clone(),
            boot.parteien.clone(),
        )
    }

    /// Every path a certificate of this deployment is configured at, read or
    /// not.
    fn pfade(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, p) in self.parties.iter().enumerate() {
            if p.as4.is_some() {
                for feld in [
                    "signing_cert_pem_file",
                    "encryption_cert_pem_file",
                    "tls_cert_pem_file",
                ] {
                    out.push(format!("party[{i}].as4.{feld}"));
                }
            }
        }
        for (i, p) in self.as4.partner.iter().enumerate() {
            out.push(format!("as4.partner[{i}].encryption_cert_pem_file"));
            for j in 0..p.signing_cert_pem_files.len() {
                out.push(format!("as4.partner[{i}].signing_cert_pem_files[{j}]"));
            }
        }
        out
    }

    /// The Prometheus exposition of `makod_certificate_expiry_days`.
    #[must_use]
    pub fn prometheus(&self, jetzt: time::OffsetDateTime) -> String {
        use std::fmt::Write as _;
        let mut out = String::from(
            "# HELP makod_certificate_expiry_days Whole days until a configured AS4 certificate \
             expires (negative once expired), labelled with its makod.toml path.\n\
             # TYPE makod_certificate_expiry_days gauge\n",
        );
        for z in self.zertifikate() {
            let tage = (z.gueltig_bis - jetzt).whole_days();
            let _ = writeln!(
                out,
                "makod_certificate_expiry_days{{pfad=\"{}\"}} {tage}",
                z.pfad.replace('\\', "\\\\").replace('"', "\\\"")
            );
        }
        out
    }

    /// Re-read the files every [`PRUEFINTERVALL`] until `shutdown`, logging
    /// every finding.
    pub fn spawn(
        self: Arc<Self>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut takt = tokio::time::interval(PRUEFINTERVALL);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = takt.tick() => {
                        self.lies();
                        for b in self.befunde(time::OffsetDateTime::now_utc()) {
                            tracing::warn!(pfad = %b.pfad, "{}", b.text);
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partner_cert(dir: &std::path::Path, tage: i64) -> std::path::PathBuf {
        use asx::testkit::pki::{KeyAlg, TestCa, Validity};
        let ca = TestCa::sm_pki();
        let now = std::time::SystemTime::now();
        let tag = std::time::Duration::from_secs(86_400);
        let not_after = if tage >= 0 {
            now + tag * u32::try_from(tage).unwrap()
        } else {
            now - tag * u32::try_from(-tage).unwrap()
        };
        let pair = ca.issue_with(
            "partner",
            KeyAlg::EcBrainpoolP256r1,
            Validity {
                not_before: now - tag * 400,
                not_after,
            },
        );
        let path = dir.join(format!("p{tage}.pem"));
        std::fs::write(&path, &pair.cert_pem).unwrap();
        path
    }

    fn as4(sig: std::path::PathBuf) -> As4Config {
        As4Config {
            partner: vec![crate::config::As4PartnerConfig {
                mp_id: "9900001000002".to_owned(),
                agency: None,
                endpoint: None,
                encryption_cert_pem_file: None,
                signing_cert_pem_files: vec![sig],
                quirks: Vec::new(),
                de0004_ausnahme: Vec::new(),
            }],
            ..As4Config::default()
        }
    }

    /// A certificate that was outside the window at boot is reported once the
    /// window reaches it — on the running daemon, without a restart — and its
    /// days left are exported.
    #[test]
    fn a_certificate_entering_the_window_is_reported_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = ZertifikatMonitor::new(&as4(partner_cert(dir.path(), 45)), &[]);
        let boot = time::OffsetDateTime::now_utc();
        let einrichtung = Einrichtung::neu(
            "makod",
            monitor.befunde(boot),
            mako_service::policy::PolicyReport::new(),
            Vec::new(),
        );
        assert!(einrichtung.befunde.is_empty(), "{:?}", einrichtung.befunde);

        let spaeter = boot + time::Duration::days(20);
        let jetzt = monitor.einrichtung(&einrichtung, spaeter);
        assert_eq!(jetzt.befunde.len(), 1, "{:?}", jetzt.befunde);
        assert_eq!(
            jetzt.befunde[0].pfad,
            "as4.partner[0].signing_cert_pem_files[0]"
        );
        assert_eq!(jetzt.befunde[0].schwere, befund::Schwere::Warnung);

        let metrik = monitor.prometheus(spaeter);
        assert!(
            metrik.contains(
                "makod_certificate_expiry_days{pfad=\"as4.partner[0].signing_cert_pem_files[0]\"} 24\n"
            ),
            "{metrik}"
        );
    }

    /// A certificate replaced on disk is read again: the expired one's
    /// blocking finding gives way to the successor's state.
    #[test]
    fn a_replaced_certificate_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let abgelaufen = partner_cert(dir.path(), -1);
        let monitor = ZertifikatMonitor::new(&as4(abgelaufen.clone()), &[]);
        let jetzt = time::OffsetDateTime::now_utc();
        let boot = Einrichtung::neu(
            "makod",
            monitor.befunde(jetzt),
            mako_service::policy::PolicyReport::new(),
            Vec::new(),
        );
        assert!(!boot.bereit);

        std::fs::copy(partner_cert(dir.path(), 365), &abgelaufen).unwrap();
        monitor.lies();
        let danach = monitor.einrichtung(&boot, jetzt);
        assert!(danach.bereit, "{:?}", danach.befunde);
    }
}
