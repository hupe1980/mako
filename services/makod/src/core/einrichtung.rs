//! makod's configuration findings — every startup check, as data.
//!
//! makod drives its own `main` rather than the `mako-service` runner, so it
//! assembles its [`Einrichtung`] here from the same checks its boot runs:
//! the `[[party]]` table ([`MpIdRegistry::pruefe`]), `[deployment] rollen`
//! ([`crate::deployment::pruefe_rollen`]), the agency each MP-ID's prefix
//! implies, adapter coverage per format version, the preflight of every door
//! ([`crate::preflight::pruefe`]), the `[marktd]` subscription, the
//! API-Webdienste signing pair and the expiry of every AS4 certificate. One
//! [`pruefe`] call feeds `--check-config`, the boot refusal and
//! `GET /admin/einrichtung`, so the three cannot disagree.

use std::collections::BTreeSet;
use std::path::Path;

use mako_service::befund::{self, Befund, Bereich};
use mako_service::einrichtung::{Dienst, Einrichtung, Partei};
use mako_service::policy::{Origin, PolicyClass, PolicyEntry, PolicyReport};

use crate::config::{As4Config, MakodPolicy, PartyConfig};
use crate::deployment::Rolle;
use crate::party_registry::{MpIdRegistry, RoleSparte, sparte_for_role};
use crate::preflight::{Preflight, PreflightInput};

/// The daemon name findings are reported under.
pub const DIENST: &str = "makod";

/// Where the Codenummer prefix rule is published.
const BILDUNGSVORSCHRIFT: &str = "BDEW-AWH Identifikatoren V1.2, Kap. 2.2 (S. 5): Stelle 1+2 \
     „99 = BDEW/Strom“, „98 = DVGW/Gas“";

/// Everything the checks read, borrowed from the parsed configuration.
pub struct Eingabe<'a> {
    /// `[[party]]`.
    pub parties: &'a [PartyConfig],
    /// `[deployment] rollen`.
    pub rollen: &'a [Rolle],
    /// The preflight input, when the `[[party]]` table built a registry.
    pub preflight: Option<&'a PreflightInput<'a>>,
    /// A `--cedar-policy-dir` that could not be read, with the reason.
    pub cedar_policy_dir_fehler: Option<String>,
    /// `[as4]`, for the certificate expiry.
    pub as4: &'a As4Config,
    /// `[policy]`.
    pub policy: &'a MakodPolicy,
    /// `[marktd] url`.
    pub marktd_url: Option<&'a str>,
    /// `[marktd] webhook_url`.
    pub marktd_webhook_url: Option<&'a str>,
    /// `--http-addr` is set.
    pub http_enabled: bool,
    /// `webdienste.signing_key_pem_file` is set.
    pub webdienste_signing_key: bool,
    /// `webdienste.signing_cert_pem_file` is set.
    pub webdienste_signing_cert: bool,
    /// The `allow_*` postures, as `(key, value)`.
    pub posture: &'a [(&'static str, bool)],
    /// When the certificates are judged.
    pub jetzt: time::OffsetDateTime,
    /// `[einrichtung]`.
    pub einrichtung: &'a crate::config::EinrichtungConfig,
}

/// What the checks found, and what boot consumes when nothing blocks.
pub struct Pruefung {
    /// The report `--check-config` prints and `/admin/einrichtung` serves.
    pub einrichtung: Einrichtung,
    /// The resolved preflight values; `Some` exactly when no preflight check
    /// blocked.
    pub preflight: Option<Preflight>,
    /// The declared deployment roles.
    pub rollen: BTreeSet<Rolle>,
    /// `[[einrichtung.dienst]]`, credentials resolved.
    pub dienste: Vec<Dienst>,
}

/// Run every startup check makod can run from its configuration.
#[must_use]
pub fn pruefe(e: &Eingabe<'_>) -> Pruefung {
    let mut befunde = MpIdRegistry::pruefe(e.parties);
    befunde.extend(codenummer_befunde(e.parties));
    let (rollen, b) = crate::deployment::pruefe_rollen(e.rollen, e.parties);
    befunde.extend(b);
    befunde.extend(crate::startup::adapter_befunde());
    if let Some(grund) = &e.cedar_policy_dir_fehler {
        befunde.push(
            Befund::blockiert("authz.cedar_policy_dir", grund.clone()).bereich(Bereich::Zugriff),
        );
    }
    let preflight = match e.preflight.map(crate::preflight::pruefe) {
        Some(Ok(p)) => Some(p),
        Some(Err(b)) => {
            befunde.extend(b);
            None
        }
        None => None,
    };
    befunde.extend(marktd_befunde(e));
    befunde.extend(signatur_paar_befunde(e));
    let fenster = e
        .as4
        .zertifikat_warnfenster_tage
        .unwrap_or(befund::ZERTIFIKAT_WARNFENSTER_TAGE);
    befunde.extend(zertifikat_befunde(e.as4, e.parties, e.jetzt, fenster));
    let (dienste, b) = dienste(e.einrichtung);
    befunde.extend(b);

    Pruefung {
        einrichtung: Einrichtung::neu(
            DIENST,
            befunde,
            policy_bericht(e.policy, &rollen, e.posture),
            parteien(e.parties),
        ),
        preflight,
        rollen,
        dienste,
    }
}

/// `[[einrichtung.dienst]]` with each credential read, and a finding per entry
/// whose credential cannot be read or is given twice.
#[must_use]
pub fn dienste(cfg: &crate::config::EinrichtungConfig) -> (Vec<Dienst>, Vec<Befund>) {
    let mut out = Vec::new();
    let mut befunde = Vec::new();
    for (i, d) in cfg.dienst.iter().enumerate() {
        let token = match (&d.api_key, &d.api_key_file) {
            (Some(_), Some(_)) => {
                befunde.push(
                    Befund::blockiert(
                        format!("einrichtung.dienst[{i}].api_key"),
                        "set `api_key` or `api_key_file`, not both",
                    )
                    .bereich(Bereich::Erreichbarkeit),
                );
                continue;
            }
            (Some(k), None) => Some(k.trim().to_owned()),
            (None, Some(f)) => match std::fs::read_to_string(f) {
                Ok(k) => Some(k.trim().to_owned()),
                Err(err) => {
                    befunde.push(
                        Befund::blockiert(
                            format!("einrichtung.dienst[{i}].api_key_file"),
                            format!("reading {}: {err}", f.display()),
                        )
                        .bereich(Bereich::Erreichbarkeit),
                    );
                    continue;
                }
            },
            (None, None) => None,
        };
        out.push(Dienst {
            name: d.name.clone(),
            url: d.url.clone(),
            token: token.map(secrecy::SecretString::from),
        });
    }
    (out, befunde)
}

/// The policy and posture makod runs under: `[policy]`, the declared
/// `rollen`, and every `allow_*` posture.
#[must_use]
pub fn policy_bericht(
    policy: &MakodPolicy,
    rollen: &BTreeSet<Rolle>,
    posture: &[(&'static str, bool)],
) -> PolicyReport {
    let mut report = policy.report();
    report.entries.push(PolicyEntry {
        key: "deployment.rollen".to_owned(),
        value: rollen
            .iter()
            .map(|r| r.token())
            .collect::<Vec<_>>()
            .join(","),
        origin: Origin::Explicit,
        class: PolicyClass::Posture,
        quelle: None,
        doc: None,
        insecure: false,
        review_by: None,
        review_grund: None,
    });
    for (key, value) in posture {
        report = report.posture_flag(key, *value);
    }
    report
}

/// One [`Partei`] per `[[party]]` role, with its Sparte where the role has one.
#[must_use]
pub fn parteien(parties: &[PartyConfig]) -> Vec<Partei> {
    let mut out = Vec::new();
    for p in parties {
        for role in &p.roles {
            let sparte = match sparte_for_role(role) {
                Some(RoleSparte::Strom) => Some("strom".to_owned()),
                Some(RoleSparte::Gas) => Some("gas".to_owned()),
                _ => None,
            };
            out.push(Partei {
                rolle: role.to_uppercase(),
                sparte,
                mp_id: p.mp_id.clone(),
            });
        }
    }
    out
}

/// A BDEW-Codenummer (`99…`) is Strom's and a DVGW-Codenummer (`98…`) Gas's:
/// a Gas role under a `99…` code, or a Strom role under a `98…` code, names
/// the operator with a code of the other Sparte. A GS1 GLN or an EIC carries
/// no Sparte in its prefix and is not judged here.
#[must_use]
pub fn codenummer_befunde(parties: &[PartyConfig]) -> Vec<Befund> {
    let mut out = Vec::new();
    for (i, p) in parties.iter().enumerate() {
        if p.mp_id.len() != 13 {
            continue;
        }
        let (code, falsche) = if p.mp_id.starts_with("99") {
            ("BDEW-Codenummer (Strom)", RoleSparte::Gas)
        } else if p.mp_id.starts_with("98") {
            ("DVGW-Codenummer (Gas)", RoleSparte::Strom)
        } else {
            continue;
        };
        let rollen: Vec<&str> = p
            .roles
            .iter()
            .filter(|r| sparte_for_role(r) == Some(falsche))
            .map(String::as_str)
            .collect();
        if rollen.is_empty() {
            continue;
        }
        out.push(
            Befund::blockiert(
                format!("party[{i}].mp_id"),
                format!(
                    "{:?} ist eine {code}, aber die Rollen {rollen:?} gehören zur anderen \
                     Sparte — eine Gas-Rolle braucht eine DVGW-Codenummer (98…), eine \
                     Strom-Rolle eine BDEW-Codenummer (99…)",
                    p.mp_id
                ),
            )
            .bereich(Bereich::Identitaeten)
            .quelle(BILDUNGSVORSCHRIFT),
        );
    }
    out
}

/// The MaLo-Ident cache follows marktd through the fan-out, which delivers to
/// the HTTP port. A marktd without the subscription address would leave the
/// cache answering counterparties from whatever it last held.
fn marktd_befunde(e: &Eingabe<'_>) -> Vec<Befund> {
    let mut out = Vec::new();
    if e.marktd_url.is_none() {
        return out;
    }
    if e.marktd_webhook_url.is_none() {
        out.push(
            Befund::blockiert(
                "marktd.webhook_url",
                format!(
                    "[marktd] url is set without [marktd] webhook_url (MAKOD_MARKTD_WEBHOOK_URL): \
                     makod's MaLo cache follows marktd through its fan-out and needs the address \
                     marktd delivers to, e.g. http://makod:8080{}",
                    crate::malo_sync::WEBHOOK_PATH
                ),
            )
            .bereich(Bereich::Erreichbarkeit),
        );
    }
    if !e.http_enabled {
        out.push(
            Befund::blockiert(
                "http.addr",
                format!(
                    "[marktd] webhook_url needs the HTTP port (--http-addr): the fan-out delivers \
                     to {}",
                    crate::malo_sync::WEBHOOK_PATH
                ),
            )
            .bereich(Bereich::Erreichbarkeit),
        );
    }
    out
}

/// The API-Webdienste signing certificate travels beside every signed
/// request, so a key needs its certificate and a certificate its key.
fn signatur_paar_befunde(e: &Eingabe<'_>) -> Vec<Befund> {
    let quelle = "Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3";
    match (e.webdienste_signing_key, e.webdienste_signing_cert) {
        (true, false) => vec![
            Befund::blockiert(
                "webdienste.signing_cert_pem_file",
                "webdienste.signing_key_pem is set without webdienste.signing_cert_pem: the \
                 certificate travels as X-BDEW-CERT beside every signed request",
            )
            .bereich(Bereich::Zertifikate)
            .quelle(quelle),
        ],
        (false, true) => vec![
            Befund::blockiert(
                "webdienste.signing_key_pem_file",
                "webdienste.signing_cert_pem is set without webdienste.signing_key_pem",
            )
            .bereich(Bereich::Zertifikate),
        ],
        _ => Vec::new(),
    }
}

/// Every `[party.as4]` and `[[as4.partner]]` certificate that has expired
/// (blocking) or expires inside `fenster` days (a warning), with its date.
///
/// A file that does not read or parse is not judged here; the AS4 preflight
/// reports it.
#[must_use]
pub fn zertifikat_befunde(
    as4: &As4Config,
    parties: &[PartyConfig],
    jetzt: time::OffsetDateTime,
    fenster: u32,
) -> Vec<Befund> {
    let mut dateien: Vec<(String, String, &Path)> = Vec::new();
    for (i, p) in parties.iter().enumerate() {
        let Some(k) = &p.as4 else { continue };
        for (feld, was, pfad) in [
            (
                "signing_cert_pem_file",
                "Signaturzertifikat",
                &k.signing_cert_pem_file,
            ),
            (
                "encryption_cert_pem_file",
                "Verschlüsselungszertifikat",
                &k.encryption_cert_pem_file,
            ),
            ("tls_cert_pem_file", "TLS-Zertifikat", &k.tls_cert_pem_file),
        ] {
            dateien.push((
                format!("party[{i}].as4.{feld}"),
                format!("Das {was} von {}", p.mp_id),
                pfad.as_path(),
            ));
        }
    }
    for (i, p) in as4.partner.iter().enumerate() {
        if let Some(f) = &p.encryption_cert_pem_file {
            dateien.push((
                format!("as4.partner[{i}].encryption_cert_pem_file"),
                format!("Das Verschlüsselungszertifikat des Partners {}", p.mp_id),
                f.as_path(),
            ));
        }
        for (j, f) in p.signing_cert_pem_files.iter().enumerate() {
            dateien.push((
                format!("as4.partner[{i}].signing_cert_pem_files[{j}]"),
                format!("Das Signaturzertifikat des Partners {}", p.mp_id),
                f.as_path(),
            ));
        }
    }
    dateien
        .into_iter()
        .filter_map(|(pfad, was, datei)| {
            let pem = std::fs::read(datei).ok()?;
            let cert = asx::Certificate::from_pem(&pem).ok()?;
            let bis = gueltig_bis(cert.der())?;
            befund::zertifikat_ablauf(&pfad, &was, bis, jetzt, fenster)
        })
        .collect()
}

// ── The policy report across boots ──────────────────────────────────────────

/// Where makod keeps the policy report it last booted with.
const POLICY: mako_engine::store_slatedb::KvNamespace =
    mako_engine::store_slatedb::KvNamespace::new("pol/");

/// Every entry of `neu` whose value differs from `alt` — all of them when
/// nothing was recorded before.
#[must_use]
pub fn policy_aenderungen(
    alt: Option<&PolicyReport>,
    neu: &PolicyReport,
) -> Vec<mako_service::policy::PolicyChange> {
    neu.entries
        .iter()
        .filter_map(|e| {
            let old = alt.and_then(|a| a.get(&e.key)).map(|o| o.value.clone());
            (old.as_deref() != Some(e.value.as_str())).then(|| mako_service::policy::PolicyChange {
                key: e.key.clone(),
                old,
                new: e.value.clone(),
            })
        })
        .collect()
}

/// Compare `neu` with the report stored at the last boot, store `neu` when
/// anything changed, and return what changed — makod's counterpart of the
/// runner's `service_policy_log`, kept in its engine store.
///
/// # Errors
///
/// A store failure.
pub async fn policy_aufzeichnen(
    store: &mako_engine::store_slatedb::SlateDbStore,
    neu: &PolicyReport,
) -> Result<Vec<mako_service::policy::PolicyChange>, mako_engine::error::EngineError> {
    let alt: Option<PolicyReport> = store
        .kv_get(POLICY, DIENST)
        .await?
        .and_then(|b| serde_json::from_slice(&b).ok());
    let aenderungen = policy_aenderungen(alt.as_ref(), neu);
    if !aenderungen.is_empty()
        && let Ok(bytes) = serde_json::to_vec(neu)
    {
        store.kv_put(POLICY, DIENST, &bytes).await?;
    }
    Ok(aenderungen)
}

// ── DER: the certificate's notAfter ─────────────────────────────────────────

/// One DER TLV at the front of `b`: (tag, header length, total length).
fn tlv(b: &[u8]) -> Option<(u8, usize, usize)> {
    let tag = *b.first()?;
    let first = *b.get(1)?;
    let (len, hdr) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | usize::from(*b.get(2 + i)?);
        }
        (len, 2 + n)
    };
    let total = hdr.checked_add(len)?;
    (total <= b.len()).then_some((tag, hdr, total))
}

/// The `notAfter` of a DER X.509 certificate (RFC 5280 §4.1.2.5): inside the
/// `tbsCertificate`, after the optional `[0]` version, serialNumber,
/// signature and issuer, `validity ::= SEQUENCE { notBefore, notAfter }`.
#[must_use]
pub fn gueltig_bis(der: &[u8]) -> Option<time::OffsetDateTime> {
    let (tag, hdr, _) = tlv(der)?;
    if tag != 0x30 {
        return None;
    }
    let cert = &der[hdr..];
    let (tag, hdr, total) = tlv(cert)?;
    if tag != 0x30 {
        return None;
    }
    let mut tbs = &cert[hdr..total];
    if tbs.first() == Some(&0xa0) {
        let (_, _, t) = tlv(tbs)?;
        tbs = &tbs[t..];
    }
    // serialNumber, signature, issuer.
    for _ in 0..3 {
        let (_, _, t) = tlv(tbs)?;
        tbs = &tbs[t..];
    }
    let (tag, hdr, total) = tlv(tbs)?;
    if tag != 0x30 {
        return None;
    }
    let mut validity = &tbs[hdr..total];
    let (_, _, t) = tlv(validity)?;
    validity = &validity[t..];
    let (tag, hdr, total) = tlv(validity)?;
    asn1_zeit(tag, &validity[hdr..total])
}

/// RFC 5280 §4.1.2.5.1 UTCTime `YYMMDDHHMMSSZ` (YY ≥ 50 is 19YY) or
/// §4.1.2.5.2 GeneralizedTime `YYYYMMDDHHMMSSZ`.
fn asn1_zeit(tag: u8, wert: &[u8]) -> Option<time::OffsetDateTime> {
    let text = std::str::from_utf8(wert).ok()?.strip_suffix('Z')?;
    let (jahr, rest) = match tag {
        0x17 if text.len() == 12 => {
            let yy: i32 = text.get(..2)?.parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &text[2..])
        }
        0x18 if text.len() == 14 => (text.get(..4)?.parse().ok()?, &text[4..]),
        _ => return None,
    };
    let zahl = |r: std::ops::Range<usize>| rest.get(r)?.parse::<u8>().ok();
    let monat = time::Month::try_from(zahl(0..2)?).ok()?;
    let datum = time::Date::from_calendar_date(jahr, monat, zahl(2..4)?).ok()?;
    let uhr = time::Time::from_hms(zahl(4..6)?, zahl(6..8)?, zahl(8..10)?).ok()?;
    Some(time::PrimitiveDateTime::new(datum, uhr).assume_utc())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use asx::testkit::pki::{KeyAlg, TestCa, Validity};

    use super::*;

    fn party(mp_id: &str, roles: &[&str]) -> PartyConfig {
        PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            primary: false,
            agency: None,
            as4: None,
        }
    }

    static EINRICHTUNG: crate::config::EinrichtungConfig =
        crate::config::EinrichtungConfig { dienst: Vec::new() };

    fn eingabe<'a>(
        parties: &'a [PartyConfig],
        rollen: &'a [Rolle],
        as4: &'a As4Config,
        policy: &'a MakodPolicy,
    ) -> Eingabe<'a> {
        Eingabe {
            parties,
            rollen,
            preflight: None,
            cedar_policy_dir_fehler: None,
            as4,
            policy,
            marktd_url: None,
            marktd_webhook_url: None,
            http_enabled: true,
            webdienste_signing_key: false,
            webdienste_signing_cert: false,
            posture: &[("cedar_permit_all", true)],
            jetzt: time::OffsetDateTime::now_utc(),
            einrichtung: &EINRICHTUNG,
        }
    }

    /// Two independent defects — a Gas role under a BDEW-Codenummer and a
    /// `[marktd] url` without its webhook — are two findings, each at its path.
    #[test]
    fn a_config_with_two_defects_reports_both() {
        let parties = [party("9900001000001", &["GNB"])];
        let as4 = As4Config::default();
        let policy = MakodPolicy::default();
        let e = Eingabe {
            marktd_url: Some("http://marktd:8180"),
            ..eingabe(&parties, &[Rolle::NbGas], &as4, &policy)
        };
        let p = pruefe(&e);
        assert!(!p.einrichtung.bereit);
        let pfade: Vec<&str> = p
            .einrichtung
            .befunde
            .iter()
            .map(|b| b.pfad.as_str())
            .collect();
        assert!(pfade.contains(&"party[0].mp_id"), "{pfade:?}");
        assert!(pfade.contains(&"marktd.webhook_url"), "{pfade:?}");
        let ident = p
            .einrichtung
            .befunde
            .iter()
            .find(|b| b.pfad == "party[0].mp_id")
            .unwrap();
        assert_eq!(ident.bereich, Bereich::Identitaeten);
        assert!(ident.quelle.as_deref().unwrap().contains("Kap. 2.2"));
        // The report carries the identities and the posture.
        assert_eq!(p.einrichtung.parteien[0].sparte.as_deref(), Some("gas"));
        assert_eq!(
            p.einrichtung
                .policy
                .get("deployment.rollen")
                .map(|e| e.value.as_str()),
            Some("nb-gas")
        );
        assert!(
            p.einrichtung
                .policy
                .get("cedar_permit_all")
                .is_some_and(|e| e.insecure)
        );
    }

    #[test]
    fn a_matching_codenummer_or_a_gln_is_no_finding() {
        assert!(
            codenummer_befunde(&[
                party("9800001000001", &["GNB"]),
                party("9900001000001", &["NB", "RB"]),
                party("4012345000023", &["LFG"]),
            ])
            .is_empty()
        );
        assert_eq!(
            codenummer_befunde(&[party("9800001000001", &["LF"])]).len(),
            1
        );
    }

    /// An expired AS4 certificate blocks; one expiring in ten days warns
    /// with its date; one valid beyond the window says nothing.
    #[test]
    fn a_certificate_expiring_in_ten_days_warns_and_an_expired_one_blocks() {
        let ca = TestCa::sm_pki();
        let now = SystemTime::now();
        let dir = tempfile::tempdir().unwrap();
        let schreibe = |name: &str, tage_bis: i64| {
            let not_after = if tage_bis >= 0 {
                now + Duration::from_secs(tage_bis.unsigned_abs() * 86_400)
            } else {
                now - Duration::from_secs(tage_bis.unsigned_abs() * 86_400)
            };
            let pair = ca.issue_with(
                name,
                KeyAlg::EcBrainpoolP256r1,
                Validity {
                    not_before: now - Duration::from_secs(400 * 86_400),
                    not_after,
                },
            );
            let path = dir.path().join(format!("{name}.pem"));
            std::fs::write(&path, &pair.cert_pem).unwrap();
            path
        };
        let as4 = As4Config {
            partner: vec![crate::config::As4PartnerConfig {
                mp_id: "9900001000002".to_owned(),
                agency: None,
                endpoint: None,
                encryption_cert_pem_file: Some(schreibe("abgelaufen", -1)),
                signing_cert_pem_files: vec![schreibe("bald", 10), schreibe("lange", 365)],
                quirks: Vec::new(),
            }],
            ..As4Config::default()
        };
        let jetzt = time::OffsetDateTime::now_utc();
        let befunde = zertifikat_befunde(&as4, &[], jetzt, 30);
        assert_eq!(befunde.len(), 2, "{befunde:?}");
        assert_eq!(befunde[0].pfad, "as4.partner[0].encryption_cert_pem_file");
        assert!(befunde[0].ist_blockiert());
        assert_eq!(befunde[1].pfad, "as4.partner[0].signing_cert_pem_files[0]");
        assert_eq!(befunde[1].schwere, befund::Schwere::Warnung);
        let datum = (jetzt + time::Duration::days(10)).date().to_string();
        assert!(befunde[1].text.contains(&datum), "{}", befunde[1].text);
        // A narrower window leaves the ten-day certificate unflagged.
        assert_eq!(zertifikat_befunde(&as4, &[], jetzt, 5).len(), 1);
    }

    /// A changed value is a change; an unchanged one is not; the first boot
    /// records everything.
    #[test]
    fn the_policy_diff_names_what_changed() {
        let alt = PolicyReport::new().posture_flag("cedar_permit_all", false);
        let neu = PolicyReport::new()
            .posture_flag("cedar_permit_all", true)
            .posture_flag("allow_volatile", false);
        let d = policy_aenderungen(Some(&alt), &neu);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].old.as_deref(), Some("false"));
        assert_eq!(d[0].new, "true");
        assert_eq!(d[1].old, None);
        assert!(policy_aenderungen(Some(&neu), &neu).is_empty());
        assert_eq!(policy_aenderungen(None, &neu).len(), 2);
    }

    #[test]
    fn utc_and_generalized_time_both_read() {
        assert_eq!(
            asn1_zeit(0x17, b"261109120000Z"),
            Some(time::macros::datetime!(2026-11-09 12:00 UTC))
        );
        assert_eq!(
            asn1_zeit(0x17, b"991231235959Z"),
            Some(time::macros::datetime!(1999-12-31 23:59:59 UTC))
        );
        assert_eq!(
            asn1_zeit(0x18, b"20500101000000Z"),
            Some(time::macros::datetime!(2050-01-01 00:00 UTC))
        );
        assert_eq!(asn1_zeit(0x17, b"2611091200Z"), None);
    }
}
