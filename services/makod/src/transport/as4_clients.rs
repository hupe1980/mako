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

use std::collections::BTreeMap;
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
                agency: registry.agency_for_mp_id(&party.mp_id).to_owned(),
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
                tls: key_pair(&at("tls"), &keys.tls_cert_pem_file, &keys.tls_key_pem_file)?,
            });
        }

        let mut partners = PartnerDirectory::default();
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
                agency: p
                    .agency
                    .clone()
                    .unwrap_or_else(|| registry.agency_for_mp_id(&p.mp_id).to_owned()),
                endpoint: p.endpoint.clone(),
                encryption_cert,
                signing_certs,
                quirks,
            })?;
        }

        Ok(Some(Self {
            domain,
            trust_anchor,
            crl_signer,
            identities,
            partners,
            unbekannte_absender,
        }))
    }
}

/// The built AS4 clients, by local MP-ID. Cheap to clone.
#[derive(Debug, Clone)]
pub struct As4Clients(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    clients: BTreeMap<String, asx::as4::As4>,
    partners: PartnerDirectory,
    domain: MessageIdDomain,
    unbekannte_absender: UnbekannteAbsender,
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
        Self::build_with(setup, |b| b)
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
    pub fn build_with_http<H: asx::HttpClient + Clone>(
        setup: As4Setup,
        http: H,
    ) -> anyhow::Result<Self> {
        Self::build_with(setup, move |b| b.http_client(http.clone()))
    }

    fn build_with(
        setup: As4Setup,
        configure: impl Fn(asx::Builder<asx::as4::As4>) -> asx::Builder<asx::as4::As4>,
    ) -> anyhow::Result<Self> {
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
        Ok(Self(Arc::new(Inner {
            clients,
            partners: setup.partners,
            domain: setup.domain,
            unbekannte_absender: setup.unbekannte_absender,
        })))
    }

    /// The client of local MP-ID `mp_id`.
    #[must_use]
    pub fn client(&self, mp_id: &str) -> Option<&asx::as4::As4> {
        self.0.clients.get(mp_id)
    }

    /// Every local MP-ID with its client.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &asx::as4::As4)> {
        self.0.clients.iter().map(|(id, c)| (id.as_str(), c))
    }

    /// The partner directory.
    #[must_use]
    pub fn partners(&self) -> &PartnerDirectory {
        &self.0.partners
    }

    /// The `eb:MessageId` of outbox entry `outbox_id`.
    #[must_use]
    pub fn message_id(&self, outbox_id: impl std::fmt::Display) -> String {
        self.0.domain.message_id(outbox_id)
    }

    /// Whether a message from `sender` may be acted on under
    /// `[policy.as4] unbekannte_absender`.
    #[must_use]
    pub fn admits_sender(&self, sender: &str) -> bool {
        match self.0.unbekannte_absender {
            UnbekannteAbsender::NurKonfigurierte => self.0.partners.contains(sender),
            UnbekannteAbsender::BdewOuZulassen => true,
        }
    }
}
