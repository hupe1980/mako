//! The AS4 clients of a deployment: one `asx::as4::As4` per local MP-ID.
//!
//! A receiving MSH checks that the signing certificate's subject OU carries
//! the `eb:From` MP-ID (BDEW AS4-Profil 1.2 §2.3.2, §2.3.4.4; Regelungen zum
//! Übertragungsweg für AS4 2.6 Kap. 5.2), and an inbox answers only for its own
//! `eb:To`. So each `[[party]]` sends with, and is served by, a client built
//! from its own `[party.as4]` key pairs. All clients share the trust anchor
//! (the SM-PKI root, nothing else), the CRL signer (CCRL-S(Root)) and the
//! `[[as4.partner]]` directory.
//!
//! [`As4Setup::from_config`] reads and checks the configuration;
//! [`As4Clients::build`] runs `As4::builder(..).build()` for every MP-ID,
//! which is the startup check of the key material — `--check` runs it too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use mako_as4::{LocalIdentity, MessageIdDomain, PartnerDirectory, PartnerEntry};

use crate::config::{As4Config, PartyConfig, UnbekannteAbsender};
use crate::party_registry::MpIdRegistry;

/// The AS4 configuration, read and checked, ready to build the clients from.
#[derive(Debug, Clone)]
pub struct As4Setup {
    /// The `id-right` of every message id sent.
    pub domain: MessageIdDomain,
    /// The SM-PKI root.
    pub trust_anchor: asx::Certificate,
    /// The CCRL-S(Root) certificate.
    pub crl_signer: asx::Certificate,
    /// One identity per `[[party]]`.
    pub identities: Vec<LocalIdentity>,
    /// Every `[[as4.partner]]`.
    pub partners: PartnerDirectory,
    /// `[policy.as4] unbekannte_absender`.
    pub unbekannte_absender: UnbekannteAbsender,
    /// `[[as4.partner]] de0004_ausnahme`: per authenticated partner, the
    /// further MP-IDs it may name in `UNB` DE 0004.
    pub de0004_ausnahmen: BTreeMap<String, BTreeSet<String>>,
}

fn read(field: &str, path: &Path) -> anyhow::Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("{field}: reading {}", path.display()))
}

fn certificate(field: &str, path: &Path) -> anyhow::Result<asx::Certificate> {
    asx::Certificate::from_pem(&read(field, path)?)
        .map_err(|e| anyhow::anyhow!("{field} ({}): {e}", path.display()))
}

fn key_pair(field: &str, cert: &Path, key: &Path) -> anyhow::Result<asx::KeyPair> {
    asx::KeyPair::from_pem(
        &read(&format!("{field}_cert_pem_file"), cert)?,
        &read(&format!("{field}_key_pem_file"), key)?,
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "{field}: the certificate {} and the key {} are not a usable pair: {e}",
            cert.display(),
            key.display()
        )
    })
}

impl As4Setup {
    /// Read the AS4 configuration.
    ///
    /// `Ok(None)` when no `[[party]]` carries `[party.as4]` — AS4 is not
    /// configured. Once one does, every party must, and `[as4]` must name the
    /// message-id domain, the trust anchor and the CRL signer.
    ///
    /// # Errors
    ///
    /// The first missing or unreadable setting, named.
    pub fn from_config(
        as4: &As4Config,
        parties: &[PartyConfig],
        registry: &MpIdRegistry,
        unbekannte_absender: UnbekannteAbsender,
    ) -> anyhow::Result<Option<Self>> {
        if as4.lenient_receipts.is_some() {
            anyhow::bail!(crate::config::LENIENT_RECEIPTS_REMOVED);
        }
        if parties.iter().all(|p| p.as4.is_none()) {
            anyhow::ensure!(
                as4.partner.is_empty(),
                "[[as4.partner]] is configured but no [[party]] has [party.as4] key material, \
                 so there is no AS4 identity to exchange messages with the partners"
            );
            return Ok(None);
        }
        let without: Vec<&str> = parties
            .iter()
            .filter(|p| p.as4.is_none())
            .map(|p| p.mp_id.as_str())
            .collect();
        anyhow::ensure!(
            without.is_empty(),
            "[[party]] {without:?} has no [party.as4]: every MP-ID sends and receives AS4 with \
             its own signing, encryption and TLS key pairs (BDEW AS4-Profil 1.2 §2.3.4.4; the \
             signing certificate's OU carries the MP-ID, Regelungen zum Übertragungsweg für AS4 \
             2.6 Kap. 5.2)"
        );
        let domain =
            MessageIdDomain::parse(as4.message_id_domain.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "as4.message_id_domain is not set: every eb:MessageId is <outbox id>@<domain> \
                 (RFC 2822 §3.6.4), so name a domain the operator controls"
                )
            })?)?;
        let trust_anchor = certificate(
            "as4.trust_anchor_pem_file",
            as4.trust_anchor_pem_file.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "as4.trust_anchor_pem_file is not set: the SM-PKI root is the one trust \
                     anchor every partner and every local signer chains to"
                )
            })?,
        )?;
        let crl_signer = certificate(
            "as4.crl_signer_pem_file",
            as4.crl_signer_pem_file.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "as4.crl_signer_pem_file is not set: the SM-PKI CCRL-S(Root) signs the \
                     Root-CA's CRL (BSI TR-03109-4 §4.2.1), and without it no certificate \
                     below the root passes the revocation check"
                )
            })?,
        )?;

        let mut identities = Vec::with_capacity(parties.len());
        for party in parties {
            let Some(keys) = &party.as4 else { continue };
            let at = |what: &str| format!("[party.as4] of {}: {what}", party.mp_id);
            identities.push(LocalIdentity {
                mp_id: party.mp_id.clone(),
                agency: registry
                    .agency_for_mp_id(&party.mp_id)
                    .ok_or_else(|| anyhow::anyhow!("[[party]] {} is no MP-ID", party.mp_id))?
                    .to_owned(),
                signer: key_pair(
                    &at("signing"),
                    &keys.signing_cert_pem_file,
                    &keys.signing_key_pem_file,
                )?,
                decrypter: key_pair(
                    &at("encryption"),
                    &keys.encryption_cert_pem_file,
                    &keys.encryption_key_pem_file,
                )?,
                retired_decrypters: Vec::new(),
                tls: key_pair(&at("tls"), &keys.tls_cert_pem_file, &keys.tls_key_pem_file)?,
            });
        }

        let mut partners = PartnerDirectory::default();
        let mut de0004_ausnahmen = BTreeMap::new();
        for p in &as4.partner {
            let at = |what: &str| format!("[[as4.partner]] {}: {what}", p.mp_id);
            anyhow::ensure!(
                !registry.is_own_mp_id(&p.mp_id),
                "[[as4.partner]] {} is one of this deployment's own MP-IDs; a self-addressed \
                 message is delivered in-process, never over AS4",
                p.mp_id
            );
            let encryption_cert = p
                .encryption_cert_pem_file
                .as_deref()
                .map(|f| certificate(&at("encryption_cert_pem_file"), f))
                .transpose()?;
            let signing_certs = p
                .signing_cert_pem_files
                .iter()
                .map(|f| certificate(&at("signing_cert_pem_files"), f))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let quirks = p
                .quirks
                .iter()
                .map(|q| mako_as4::parse_quirk(&p.mp_id, q))
                .collect::<Result<Vec<_>, _>>()?;
            partners.insert(PartnerEntry {
                mp_id: p.mp_id.clone(),
                agency: match &p.agency {
                    Some(a) => a.clone(),
                    None => registry
                        .agency_for_mp_id(&p.mp_id)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "[[as4.partner]] {} is no MP-ID: the AS4 PartyId is a \
                                 BDEW-/DVGW-Codenummer or GLN (BDEW AS4-Profil 1.2 §2.3.1.1, \
                                 S. 13–14), all 13 digits",
                                p.mp_id
                            )
                        })?
                        .to_owned(),
                },
                endpoint: p.endpoint.clone(),
                encryption_cert,
                signing_certs,
                quirks,
            })?;
            for vertreten in &p.de0004_ausnahme {
                anyhow::ensure!(
                    vertreten.len() == 13 && vertreten.bytes().all(|b| b.is_ascii_digit()),
                    "[[as4.partner]] {}: de0004_ausnahme {vertreten:?} is not an MP-ID — UNB \
                     DE 0004 carries a 13-digit BDEW-/DVGW-Codenummer or GLN (Allgemeine \
                     Festlegungen 6.1d Kap. 2.13, S. 17)",
                    p.mp_id
                );
                anyhow::ensure!(
                    *vertreten != p.mp_id && !registry.is_own_mp_id(vertreten),
                    "[[as4.partner]] {}: de0004_ausnahme {vertreten:?} names the partner itself \
                     or one of this deployment's own MP-IDs",
                    p.mp_id
                );
            }
            if !p.de0004_ausnahme.is_empty() {
                de0004_ausnahmen.insert(
                    p.mp_id.clone(),
                    p.de0004_ausnahme.iter().cloned().collect::<BTreeSet<_>>(),
                );
            }
        }

        Ok(Some(Self {
            domain,
            trust_anchor,
            crl_signer,
            identities,
            partners,
            unbekannte_absender,
            de0004_ausnahmen,
        }))
    }
}

/// The built AS4 clients, by local MP-ID. Cheap to clone; every clone sees a
/// [`Self::rotate`] at once.
#[derive(Clone)]
pub struct As4Clients(Arc<Shared>);

type Configure = dyn Fn(asx::Builder<asx::as4::As4>) -> asx::Builder<asx::as4::As4> + Send + Sync;

struct Shared {
    current: std::sync::RwLock<Arc<Inner>>,
    generation: std::sync::atomic::AtomicU64,
    configure: Box<Configure>,
}

impl std::fmt::Debug for As4Clients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("As4Clients")
            .field("generation", &self.generation())
            .field("mp_ids", &self.mp_ids())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct Inner {
    clients: BTreeMap<String, asx::as4::As4>,
    identities: Vec<LocalIdentity>,
    /// What a rotation compares: the identities' certificates and the
    /// partner directory.
    fingerprint: String,
    partners: PartnerDirectory,
    domain: MessageIdDomain,
    unbekannte_absender: UnbekannteAbsender,
    de0004_ausnahmen: BTreeMap<String, BTreeSet<String>>,
}

/// The certificates of `setup`'s identities and its partner directory, as one
/// comparable value.
fn fingerprint(setup: &As4Setup) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for id in &setup.identities {
        for kp in [&id.signer, &id.decrypter, &id.tls] {
            let _ = write!(
                out,
                "{}:{:?};",
                id.mp_id,
                asx::Decrypter::certificate(kp).der()
            );
        }
    }
    let _ = write!(out, "{:?}", setup.partners);
    out
}

impl As4Clients {
    /// Build one client per identity with the built-in HTTP client.
    ///
    /// # Errors
    ///
    /// The `asx-rs` `Config` refusal of the first identity that does not build
    /// (naming the setting and the profile section), or two of its uses
    /// sharing a key pair.
    pub fn build(setup: As4Setup) -> anyhow::Result<Self> {
        Self::build_with(setup, Box::new(|b| b))
    }

    /// Build with `http` carrying every request — partner deliveries and CRL
    /// fetches. For tests over an in-process loopback.
    ///
    /// # Errors
    ///
    /// As [`Self::build`].
    #[allow(
        dead_code,
        reason = "the integration tests build over a loopback through the library target"
    )]
    pub fn build_with_http<H: asx::HttpClient + Clone + Send + Sync + 'static>(
        setup: As4Setup,
        http: H,
    ) -> anyhow::Result<Self> {
        Self::build_with(setup, Box::new(move |b| b.http_client(http.clone())))
    }

    fn build_with(setup: As4Setup, configure: Box<Configure>) -> anyhow::Result<Self> {
        let inner = Self::inner_from(setup, &configure)?;
        Ok(Self(Arc::new(Shared {
            current: std::sync::RwLock::new(Arc::new(inner)),
            generation: std::sync::atomic::AtomicU64::new(0),
            configure,
        })))
    }

    fn inner_from(setup: As4Setup, configure: &Configure) -> anyhow::Result<Inner> {
        let fingerprint = fingerprint(&setup);
        let mut clients = BTreeMap::new();
        for identity in &setup.identities {
            let builder = mako_as4::client_builder(
                identity,
                setup.trust_anchor.clone(),
                setup.crl_signer.clone(),
                &setup.partners,
            )?;
            let client = configure(builder).build().map_err(|e| {
                anyhow::anyhow!(
                    "the AS4 identity of [[party]] {} does not build: {e}",
                    identity.mp_id
                )
            })?;
            clients.insert(identity.mp_id.clone(), client);
        }
        Ok(Inner {
            clients,
            identities: setup.identities,
            fingerprint,
            partners: setup.partners,
            domain: setup.domain,
            unbekannte_absender: setup.unbekannte_absender,
            de0004_ausnahmen: setup.de0004_ausnahmen,
        })
    }

    fn inner(&self) -> Arc<Inner> {
        Arc::clone(
            &self
                .0
                .current
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// How many rotations these clients have been through.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.0.generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Replace the clients with those `setup` builds, without a restart, when
    /// its key material or partners differ from the running ones; `Ok(false)`
    /// when nothing changed.
    ///
    /// The new signing key signs at once: the signing certificate travels in
    /// every message (Regelungen zum Übertragungsweg für AS4 2.6, Kap. 5.3,
    /// S. 6: „Der öffentliche Schlüssel zum Signieren … darf daher vom
    /// Absender einer AS4-Nachricht sofort verwendet werden"). A replaced
    /// encryption key pair keeps decrypting until its certificate expires,
    /// because a partner may still encrypt to it inside the overlap of at least
    /// 10 Werktage the same chapter sets.
    ///
    /// # Errors
    ///
    /// An identity of `setup` that does not build — the running clients stay.
    pub fn rotate(&self, mut setup: As4Setup, now: time::OffsetDateTime) -> anyhow::Result<bool> {
        let old = self.inner();
        if fingerprint(&setup) == old.fingerprint {
            return Ok(false);
        }
        for identity in &mut setup.identities {
            let Some(previous) = old.identities.iter().find(|i| i.mp_id == identity.mp_id) else {
                continue;
            };
            let current = asx::Decrypter::certificate(&identity.decrypter).clone();
            for kp in std::iter::once(&previous.decrypter).chain(&previous.retired_decrypters) {
                let cert = asx::Decrypter::certificate(kp);
                let valid =
                    crate::einrichtung::gueltig_bis(cert.der()).is_some_and(|bis| bis > now);
                let known = mako_as4::keys::same_public_key(cert, &current)
                    || identity.retired_decrypters.iter().any(|r| {
                        mako_as4::keys::same_public_key(asx::Decrypter::certificate(r), cert)
                    });
                if valid && !known {
                    identity.retired_decrypters.push(kp.clone());
                }
            }
        }
        let inner = Self::inner_from(setup, &self.0.configure)?;
        *self
            .0
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(inner);
        self.0
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(true)
    }

    /// The client of local MP-ID `mp_id`, as it stands now.
    #[must_use]
    pub fn client(&self, mp_id: &str) -> Option<asx::as4::As4> {
        self.inner().clients.get(mp_id).cloned()
    }

    /// Every local MP-ID.
    #[must_use]
    pub fn mp_ids(&self) -> Vec<String> {
        self.inner().clients.keys().cloned().collect()
    }

    /// The partner directory.
    #[must_use]
    pub fn partners(&self) -> PartnerDirectory {
        self.inner().partners.clone()
    }

    /// The `eb:MessageId` of outbox entry `outbox_id`.
    #[must_use]
    pub fn message_id(&self, outbox_id: impl std::fmt::Display) -> String {
        self.inner().domain.message_id(outbox_id)
    }

    /// Whether a message from `sender` may be acted on under
    /// `[policy.as4] unbekannte_absender`.
    #[must_use]
    pub fn admits_sender(&self, sender: &str) -> bool {
        let inner = self.inner();
        match inner.unbekannte_absender {
            UnbekannteAbsender::NurKonfigurierte => inner.partners.contains(sender),
            UnbekannteAbsender::BdewOuZulassen => true,
        }
    }

    /// Whether an interchange whose `UNB` DE 0004 names `de0004` may be acted
    /// on when it arrived from the authenticated partner `from`.
    ///
    /// The MP-ID in DE 0004 is the sender's own (Allgemeine Festlegungen 6.1d
    /// Kap. 2.13, S. 17: „Die im UNB- und NAD-Segment für den Absender /
    /// Empfänger verwendeten MP-ID sind identisch."), and the AS4 sender is
    /// bound to the MP-ID in its certificate (BDEW AS4-Profil 1.2 Kap.
    /// 2.3.4.4, S. 17). So DE 0004 is `from`, unless `[[as4.partner]]
    /// de0004_ausnahme` names `de0004` for `from`.
    #[must_use]
    pub fn binds_interchange_sender(&self, from: &str, de0004: &str) -> bool {
        from == de0004
            || self
                .inner()
                .de0004_ausnahmen
                .get(from)
                .is_some_and(|vertreten| vertreten.contains(de0004))
    }
}

/// How often [`spawn_rotation`] re-reads the AS4 key material.
pub const ROTATION_PRUEFINTERVALL: std::time::Duration = std::time::Duration::from_secs(300);

/// Re-read the configuration with `reload` every [`ROTATION_PRUEFINTERVALL`]
/// and [`As4Clients::rotate`] onto it, until `shutdown`. A configuration that
/// does not read or build is logged and the running clients stay.
pub fn spawn_rotation(
    clients: As4Clients,
    reload: impl Fn() -> anyhow::Result<Option<As4Setup>> + Send + 'static,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut takt = tokio::time::interval(ROTATION_PRUEFINTERVALL);
        takt.tick().await;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = takt.tick() => {
                    let setup = match reload() {
                        Ok(Some(setup)) => setup,
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::error!(error = %e, "AS4 rotation: the configuration does not read — the running clients stay");
                            continue;
                        }
                    };
                    match clients.rotate(setup, time::OffsetDateTime::now_utc()) {
                        Ok(true) => tracing::info!(generation = clients.generation(), "AS4 rotation: new key material in use"),
                        Ok(false) => {}
                        Err(e) => tracing::error!(error = %e, "AS4 rotation: the new key material does not build — the running clients stay"),
                    }
                }
            }
        }
    })
}
