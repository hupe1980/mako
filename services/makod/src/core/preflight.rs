//! Configuration preflight — everything decidable before a socket is opened.
//!
//! # Why this module exists
//!
//! `makod --check` promises deployment pipelines one thing: *exit 0 means this
//! configuration will start*. That promise only holds if every fatal
//! configuration error is discovered **before** the check-mode exit.
//!
//! Historically several were not. A config naming an AS4 partner with no
//! encryption certificate, a partner endpoint on plain `http://`, or a
//! syntactically invalid Cedar policy all passed `--check` with exit 0 and then
//! killed the real boot — the pipeline had already promoted the release.
//!
//! Everything that can be judged from the configuration alone now lives here.
//! `main` runs [`preflight`] before the `--check` exit and hands the resolved
//! values to the workers, so the check and the boot cannot diverge: they run the
//! same code and the boot consumes the check's own output.
//!
//! # What is deliberately *not* here
//!
//! Anything requiring the network or the store: OIDC discovery, the JWKS fetch,
//! socket binds, SlateDB opening. `--check` must be runnable from a CI runner
//! with no route to the identity provider. OIDC *arguments* are still validated
//! (an issuer without an audience is rejected); only the round-trip is deferred.

use std::collections::HashMap;

use anyhow::Context as _;
use mako_as4::profile::BdewAs4Profile;
use secrecy::{ExposeSecret as _, SecretString};

use crate::cedar_authz::{CedarAuthorizer, DefaultPolicy, NamedKey};

/// Whether `url` may be used as an outbound delivery destination.
///
/// `true` for HTTPS, and for `localhost` on any scheme so a development
/// instance can point at a local stub.
///
/// # Why this is one function
///
/// Three different paths can set a MaLo-ID callback address: the
/// `--maloid-partner` flag, a record discovered from the BDEW
/// Verzeichnisdienst, and `PUT /admin/partners/{mp_id}`. Only the first was
/// checked at all, and only for parseability — so an endpoint on plain
/// `http://` reached the store through either of the other two and `makod` then
/// posted a `MaloIdentResultPositive` to it: the Marktlokation, its postal
/// address and its NB/MSB assignment, in the clear (DSGVO Art. 32). A rule
/// enforced on one of three paths is not enforced.
#[must_use]
pub fn is_secure_endpoint(url: &reqwest::Url) -> bool {
    url.scheme() == "https" || url.host_str() == Some("localhost")
}

/// The `COM` DE 3155 qualifiers whose address is a URL `makod` **delivers to**.
///
/// `AK`/`AS4` is the AS4 inbox, `AW` the API-Webdienste callback. Everything
/// else a `COM` segment can carry — `EM` e-mail, `TE` telephone, `FX` — is
/// contact data, not a delivery destination, and is stored as given.
pub const DELIVERY_CHANNEL_QUALIFIERS: &[&str] = &["AK", "AS4", "AW"];

/// `true` when `qualifier` names a channel `makod` delivers to, and `address`
/// is not a URL [`is_secure_endpoint`] accepts.
#[must_use]
pub fn is_insecure_delivery_channel(qualifier: &str, address: &str) -> bool {
    DELIVERY_CHANNEL_QUALIFIERS.contains(&qualifier)
        && !reqwest::Url::parse(address).is_ok_and(|u| is_secure_endpoint(&u))
}

/// Raw configuration values the preflight judges.
///
/// Borrowed from the parsed CLI struct in `main`. Kept as an explicit input
/// rather than `&Cli` because `Cli` belongs to the binary target and this module
/// is compiled into the library target as well.
pub struct PreflightInput<'a> {
    /// Primary Marktpartner-ID — the AS4 `<eb:From>` fallback and tenant key.
    pub primary_mp_id: &'a str,
    /// `--as4-addr` is set, so the inbound transport will start.
    pub as4_inbound_enabled: bool,
    /// `--http-addr` is set.
    pub http_enabled: bool,
    /// `--api-webdienste-addr` is set.
    pub webdienste_enabled: bool,
    /// `--webdienste-allow-unauthenticated`.
    pub webdienste_allow_unauthenticated: bool,
    /// `--as4-partner MP-ID=HTTPS-URL` pairs.
    pub as4_partner: &'a [String],
    /// `--as4-partner-cert MP-ID=<PEM>` pairs.
    pub as4_partner_cert: &'a [String],
    /// `--as4-signing-key-pem`.
    pub as4_signing_key_pem: Option<&'a SecretString>,
    /// `--as4-signing-cert-pem`.
    pub as4_signing_cert_pem: Option<&'a str>,
    /// `--as4-trust-anchor-pem`.
    pub as4_trust_anchor_pem: Option<&'a str>,
    /// `--as4-decryption-key-pem`.
    pub as4_decryption_key_pem: Option<&'a SecretString>,
    /// `--as4-party-id`.
    pub as4_party_id: Option<&'a str>,
    /// `--allow-unencrypted-as4`.
    pub allow_unencrypted_as4: bool,
    /// `--allow-no-as4-trust-anchor`.
    pub allow_no_as4_trust_anchor: bool,
    /// `--allow-no-as4-signing`.
    pub allow_no_as4_signing: bool,
    /// `--edifact-outbox-webhook-url`.
    pub edifact_outbox_webhook_url: Option<&'a str>,
    /// `--erp-webhook-url`.
    pub erp_webhook_url: Option<&'a str>,
    /// `--netzzugang-endpoint-url`.
    pub netzzugang_endpoint_url: Option<&'a str>,
    /// `--maloid-partner MP-ID=URL` pairs.
    pub maloid_partner: &'a [String],
    /// `--verzeichnisdienst-url`.
    pub verzeichnisdienst_url: Option<&'a str>,
    /// `--marktd-url`.
    pub marktd_url: Option<&'a str>,
    /// `--marktd-api-key`.
    pub marktd_api_key: Option<&'a str>,
    /// `--auth-key NAME=TOKEN` pairs.
    pub auth_keys: &'a [String],
    /// Concatenated `*.cedar` policy text from `--cedar-policy-dir`.
    pub cedar_policies: Option<String>,
    /// `--cedar-no-default-policy`.
    pub cedar_no_default_policy: bool,
    /// `--oidc-issuer`.
    pub oidc_issuer: Option<&'a str>,
    /// `--oidc-audience`.
    pub oidc_audience: Option<&'a str>,
}

/// Validated configuration, consumed by the workers and the servers.
///
/// Producing one of these is the proof that the configuration is startable.
pub struct Preflight {
    /// AS4 partner P-Mode registry with every partner endpoint and encryption
    /// certificate already registered.
    pub as4_profile: BdewAs4Profile,
    /// MaLo-ID callback endpoints, keyed by counterparty Marktpartner-ID.
    pub maloid_partners: HashMap<String, reqwest::Url>,
    /// Verzeichnisdienst base URL.
    pub verzeichnisdienst_url: Option<reqwest::Url>,
    /// Parsed named API keys.
    pub auth_keys: Vec<NamedKey>,
    /// Cedar policy text supplied by the operator.
    pub cedar_policies: Option<String>,
    /// Which built-in baseline the authorizer runs with.
    pub cedar_default_policy: DefaultPolicy,
    /// AS4 `<eb:From>/<eb:PartyId>` used for outbound messages.
    pub as4_party_id: String,
}

/// Validate the configuration and resolve it into ready-to-use values.
///
/// # Errors
///
/// Returns the first fatal configuration error. Every error here is one that
/// would otherwise abort the real boot, which is precisely why the check runs
/// them all before `--check` reports success.
pub fn preflight(input: &PreflightInput<'_>) -> anyhow::Result<Preflight> {
    let as4_party_id = input.as4_party_id.unwrap_or(input.primary_mp_id).to_owned();

    // ── Ingest transport ─────────────────────────────────────────────────────
    //
    // A daemon that can receive nothing is a misconfiguration, not a quiet
    // idle process — and it is caught here, before the workers are spawned.
    // Discovered afterwards the failure is racy and unreportable by `--check`.
    anyhow::ensure!(
        input.as4_inbound_enabled || input.http_enabled,
        "No ingest transport configured: neither --as4-addr nor --http-addr is set. \
         The engine could not receive any inbound message. Set at least one."
    );

    // ── Authenticated ports need credentials ─────────────────────────────────
    let needs_auth =
        input.http_enabled || (input.webdienste_enabled && !input.webdienste_allow_unauthenticated);
    if needs_auth && input.auth_keys.is_empty() && input.oidc_issuer.is_none() {
        anyhow::bail!(
            "--auth-key / MAKOD_AUTH_KEYS or --oidc-issuer / MAKOD_OIDC_ISSUER is \
             required when --http-addr or --api-webdienste-addr is set.\n\
             These ports perform privileged operations (submitting commands, \
             triggering migrations, API-Webdienste requests) and must not be \
             exposed unauthenticated.\n\
             Provide at least one named API key with --auth-key NAME=TOKEN \
             (e.g. --auth-key erp-prod=$(openssl rand -hex 32)), or configure \
             an OIDC issuer with --oidc-issuer <URL> --oidc-audience <AUD>."
        );
    }

    // ── OIDC arguments (no network round-trip) ───────────────────────────────
    if input.oidc_issuer.is_some() {
        anyhow::ensure!(
            input.oidc_audience.is_some(),
            "--oidc-audience / MAKOD_OIDC_AUDIENCE is required when \
             --oidc-issuer / MAKOD_OIDC_ISSUER is set"
        );
    }
    if let Some(issuer) = input.oidc_issuer {
        let url = reqwest::Url::parse(issuer)
            .with_context(|| format!("--oidc-issuer: invalid URL {issuer:?}"))?;
        anyhow::ensure!(
            url.scheme() == "https" || url.host_str() == Some("localhost"),
            "--oidc-issuer must use HTTPS (got {issuer:?}); bearer tokens would \
             otherwise be validated against keys fetched over plaintext"
        );
    }

    // ── Cedar ────────────────────────────────────────────────────────────────
    let parse_keys = || {
        input
            .auth_keys
            .iter()
            .map(|s| NamedKey::from_arg(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("{e}"))
    };
    let auth_keys = parse_keys()?;
    let cedar_default_policy = if input.cedar_no_default_policy {
        DefaultPolicy::Deny
    } else {
        DefaultPolicy::PermitAll
    };
    // Compile the policy set now. Building the real authorizer later needs an
    // OIDC verifier, which needs the network; the policy text does not, and a
    // policy that fails to parse is the failure this catches. `NamedKey` holds a
    // `SecretString` and is deliberately not `Clone`, so the throw-away
    // authorizer gets its own parse of the same arguments.
    CedarAuthorizer::new(
        parse_keys()?,
        input.cedar_policies.clone(),
        None,
        Some(input.primary_mp_id.to_owned()),
        cedar_default_policy,
    )
    .map_err(|e| anyhow::anyhow!("Cedar policy set is invalid: {e}"))?;

    // ── AS4 inbound material ─────────────────────────────────────────────────
    if input.as4_inbound_enabled {
        let key_pem = input.as4_signing_key_pem.ok_or_else(|| {
            anyhow::anyhow!(
                "--as4-signing-key-pem / MAKOD_AS4_SIGNING_KEY_PEM is required when --as4-addr is set"
            )
        })?;
        let cert_pem = input.as4_signing_cert_pem.ok_or_else(|| {
            anyhow::anyhow!(
                "--as4-signing-cert-pem / MAKOD_AS4_SIGNING_CERT_PEM is required when --as4-addr is set"
            )
        })?;

        // BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every inbound message to be
        // encrypted. Without the operator's own decryption key the inbound
        // policy cannot demand it, so this is fail-closed.
        if input.as4_decryption_key_pem.is_none() && !input.allow_unencrypted_as4 {
            anyhow::bail!(
                "AS4 inbound decryption key not configured \
                 (--as4-decryption-key-pem / MAKOD_AS4_DECRYPTION_KEY_PEM not set). \
                 BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every inbound AS4 message \
                 to be encrypted; without your own EC (BrainpoolP256r1) private key, \
                 unencrypted inbound cannot be rejected. Provide the key, or pass \
                 --allow-unencrypted-as4 for dev/test."
            );
        }

        // Counterparty certificates are issued by the BDEW/BNetzA PKI, so
        // verifying them needs that CA. With none configured the session falls
        // back to the operator's own leaf certificate, which trusts exactly one
        // signer — ourselves — and rejects every partner. That is a complete
        // inbound outage presented by a daemon that reports healthy, so it is
        // fail-closed like the encryption key above rather than a log line
        // nobody reads until a counterparty escalates.
        if !input.allow_no_as4_trust_anchor
            && input.as4_trust_anchor_pem.is_none_or(|ta| ta == cert_pem)
        {
            anyhow::bail!(
                "AS4 trust anchor not configured, or set to this operator's own \
                 signing certificate. Counterparty signing certificates are issued \
                 by the BDEW/BNetzA PKI, so every inbound AS4 message would fail \
                 signature verification and the listener would accept nothing. \
                 Set --as4-trust-anchor-pem / MAKOD_AS4_TRUST_ANCHOR_PEM (or \
                 as4.trust_anchor_pem_file) to the BDEW/BNetzA PKI CA certificate, \
                 or pass --allow-no-as4-trust-anchor for a loopback test where both \
                 ends share one certificate."
            );
        }

        // Build a throw-away session to prove the PEM material parses and the
        // key matches the certificate. Deferring this to the real boot meant a
        // malformed key was found only after `--check` had reported success.
        let trust_anchor = input.as4_trust_anchor_pem.unwrap_or(cert_pem);
        asx_rs::core::SessionContextBuilder::new("makod-preflight", &as4_party_id)
            .with_signing_material(cert_pem, key_pem.expose_secret())
            .with_trust_anchor_pem(trust_anchor)
            .build()
            .map_err(|e| {
                anyhow::anyhow!(
                    "AS4 signing material is unusable: {e}. Check that \
                     --as4-signing-cert-pem and --as4-signing-key-pem are a matching \
                     PEM certificate/key pair and that --as4-trust-anchor-pem parses."
                )
            })?;
    }

    // ── AS4 outbound: partner registry ───────────────────────────────────────
    let mut as4_profile = BdewAs4Profile::new();
    for pair in input.as4_partner {
        let (mp_id, url) = pair.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("--as4-partner: expected MP-ID=HTTPS-URL, got {pair:?}")
        })?;
        let (mp_id, url) = (mp_id.trim(), url.trim());
        anyhow::ensure!(
            !mp_id.is_empty(),
            "--as4-partner: MP-ID must not be empty in {pair:?}"
        );
        anyhow::ensure!(
            url.starts_with("https://"),
            "--as4-partner: endpoint URL must use HTTPS (got {url:?} for MP-ID {mp_id:?})"
        );
        as4_profile.register_partner_all_actions(mp_id, url);
    }

    for pair in input.as4_partner_cert {
        let (mp_id, cert_pem) = pair.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("--as4-partner-cert: expected MP-ID=<PEM>, got {pair:?}")
        })?;
        let (mp_id, cert_pem) = (mp_id.trim(), cert_pem.trim());
        anyhow::ensure!(
            !mp_id.is_empty(),
            "--as4-partner-cert: MP-ID must not be empty in {pair:?}"
        );
        anyhow::ensure!(
            cert_pem.contains("-----BEGIN CERTIFICATE-----"),
            "--as4-partner-cert: value for MP-ID {mp_id:?} is not a PEM certificate \
             (no -----BEGIN CERTIFICATE----- header). When using a file reference, \
             set as4.partner_cert_files in makod.toml."
        );
        as4_profile.register_partner_encryption_cert(mp_id, cert_pem.as_bytes().to_vec());
    }

    // Every registered partner endpoint needs an encryption certificate — the
    // send path refuses `encrypt = true` without one, so a missing certificate
    // means every delivery to that partner dead-letters. Fail closed and name
    // the MP-IDs rather than discovering it one dead letter at a time.
    let cert_mp_ids: std::collections::HashSet<&str> = input
        .as4_partner_cert
        .iter()
        .filter_map(|p| p.split_once('=').map(|(g, _)| g.trim()))
        .collect();
    let endpoint_mp_ids: std::collections::HashSet<&str> = input
        .as4_partner
        .iter()
        .filter_map(|p| p.split_once('=').map(|(g, _)| g.trim()))
        .collect();
    let missing: Vec<&str> = endpoint_mp_ids
        .iter()
        .copied()
        .filter(|g| !cert_mp_ids.contains(g))
        .collect();
    if !missing.is_empty() && !input.allow_unencrypted_as4 {
        anyhow::bail!(
            "AS4 partners registered without encryption certificates: {missing:?}. \
             BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every outbound message to be \
             encrypted with the recipient's EC (BrainpoolP256r1) certificate. \
             Add --as4-partner-cert MP-ID=<PEM> (or as4.partner_cert_files in \
             makod.toml) for each partner, or pass --allow-unencrypted-as4 for dev/test."
        );
    }

    // The mirror case: a certificate whose MP-ID has no endpoint. Nothing consumes
    // it, so the operator has configured a partner that cannot be delivered to
    // while the config reads as though they can — which is exactly how a
    // mistyped MP-ID presents. The pair is the unit of configuration, so an
    // orphaned half is an error rather than a warning.
    let orphaned: Vec<&str> = cert_mp_ids
        .iter()
        .copied()
        .filter(|g| !endpoint_mp_ids.contains(g))
        .collect();
    anyhow::ensure!(
        orphaned.is_empty(),
        "AS4 encryption certificates configured for MP-IDs with no endpoint: {orphaned:?}. \
         A certificate without a matching --as4-partner MP-ID=HTTPS-URL is never used, \
         and a mistyped MP-ID looks exactly like this. Add the endpoint, or remove the \
         certificate."
    );

    // ── Outbound delivery path ───────────────────────────────────────────────
    //
    // Signing material drives the AS4 sender; the EDIFACT webhook is the
    // development substitute. With neither, outbound EDIFACT is logged and
    // rescheduled forever, which is a silent regulatory failure.
    let has_signing = input.as4_signing_key_pem.is_some() && input.as4_signing_cert_pem.is_some();
    anyhow::ensure!(
        has_signing || input.edifact_outbox_webhook_url.is_some() || input.allow_no_as4_signing,
        "AS4 signing credentials not configured \
         (--as4-signing-key-pem / --as4-signing-cert-pem not set) and no \
         --edifact-outbox-webhook-url fallback is configured. \
         Outbound EDIFACT delivery would silently fail for all messages. \
         To suppress this error in non-production environments, pass \
         --allow-no-as4-signing."
    );

    // ── Outbound callback URLs ───────────────────────────────────────────────
    //
    // Every one of these is a delivery destination. An unparseable URL is not
    // discovered until the first message is due, and by then the failure shows
    // up as a delivery error on a regulated message rather than as the
    // configuration typo it is.
    for (flag, value) in [
        (
            "--edifact-outbox-webhook-url",
            input.edifact_outbox_webhook_url,
        ),
        ("--erp-webhook-url", input.erp_webhook_url),
        ("--netzzugang-endpoint-url", input.netzzugang_endpoint_url),
        ("--marktd-url", input.marktd_url),
    ] {
        let Some(raw) = value else { continue };
        let url =
            reqwest::Url::parse(raw).with_context(|| format!("{flag}: invalid URL {raw:?}"))?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "{flag}: expected an http(s) URL, got scheme {:?} in {raw:?}",
            url.scheme()
        );
        anyhow::ensure!(url.host_str().is_some(), "{flag}: URL has no host: {raw:?}");
    }

    // ── MaLo-ID partner directory ────────────────────────────────────────────
    let mut maloid_partners = HashMap::new();
    for pair in input.maloid_partner {
        let (mp_id, url_str) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--maloid-partner: expected MP-ID=URL, got {pair:?}"))?;
        let url = reqwest::Url::parse(url_str.trim())
            .map_err(|e| anyhow::anyhow!("--maloid-partner: invalid URL {url_str:?}: {e}"))?;
        // The AS4 partner endpoint two blocks up has always had to be HTTPS;
        // this one carries the Marktlokation's postal address and its NB/MSB
        // assignment, and was only checked for parseability.
        anyhow::ensure!(
            is_secure_endpoint(&url),
            "--maloid-partner: the callback URL for {} must use HTTPS (got {url_str:?}). \
             A MaLo-ID callback carries the Marktlokation's address and its NB/MSB \
             assignment; it is not sent in the clear.",
            mp_id.trim(),
        );
        maloid_partners.insert(mp_id.trim().to_owned(), url);
    }

    let verzeichnisdienst_url = input
        .verzeichnisdienst_url
        .map(|s| {
            reqwest::Url::parse(s)
                .map_err(|e| anyhow::anyhow!("--verzeichnisdienst-url: invalid URL {s:?}: {e}"))
        })
        .transpose()?;

    if input.marktd_url.is_some() {
        // An empty key is worse than no marktd at all. The ESA consent gate and
        // the M1 Konfigurationsprodukt guard both **fail open** on a lookup
        // error — deliberately, because a marktd outage must not reject
        // regulated traffic — so an unauthenticated client turns every check
        // into a silent pass while the operator sees marktd configured and
        // believes the gate is on.
        anyhow::ensure!(
            input.marktd_api_key.is_some_and(|k| !k.trim().is_empty()),
            "--marktd-url is set but no API key is configured. \
             marktd would reject every request, and the ESA consent gate and the \
             Konfigurationsprodukt guard fail open on a lookup error — both would \
             silently pass everything. Set marktd.api_key_file in makod.toml \
             (preferred), marktd.api_key, or --marktd-api-key."
        );
    }

    Ok(Preflight {
        as4_profile,
        maloid_partners,
        verzeichnisdienst_url,
        auth_keys,
        cedar_policies: input.cedar_policies.clone(),
        cedar_default_policy,
        as4_party_id,
    })
}

/// Emit the non-fatal configuration warnings.
///
/// Separate from [`preflight`] because these describe a *degraded* deployment
/// rather than an unstartable one — and because `--check` should report them
/// too, which is why they are not buried in the server-start branches.
pub fn warn_on_degraded_config(input: &PreflightInput<'_>, durable_store: bool) {
    if input.as4_inbound_enabled {
        // Reachable only under `--allow-no-as4-trust-anchor`; the preflight
        // refuses this configuration otherwise. Restated at boot because the
        // opt-out is easy to leave in a config file and its effect — no
        // counterparty can reach us — is otherwise invisible.
        if input
            .as4_trust_anchor_pem
            .is_none_or(|ta| Some(ta) == input.as4_signing_cert_pem)
        {
            tracing::error!(
                "--allow-no-as4-trust-anchor: the AS4 trust anchor is this operator's \
                 own signing certificate. Inbound AS4 messages from all counterparties \
                 will be REJECTED because their certificates are signed by the BDEW PKI \
                 CA, not by this operator. Never run the regulated market this way."
            );
        }
        if input.as4_decryption_key_pem.is_none() {
            tracing::warn!(
                "--allow-unencrypted-as4: AS4 inbound decryption key not configured. \
                 Inbound AS4 messages will be accepted WITHOUT verifying that they \
                 are encrypted, violating BDEW AS4-Profil v1.2 §2.2.6.2.2. Dev/test only."
            );
        }
        if !durable_store {
            tracing::warn!(
                "AS4 inbox dedup storage is volatile (in-memory): duplicate detection \
                 is lost on restart. Set --data-dir / MAKOD_DATA_DIR to a persistent \
                 path to enable durable dedup (required for BDEW AS4 conformance)."
            );
        }
    } else {
        tracing::warn!(
            "AS4 inbound transport is NOT configured \
             (--as4-addr / MAKOD_AS4_ADDR unset). \
             BDEW EDIFACT messages cannot be received via the mandatory AS4 \
             transport. Set --as4-addr and provide signing key/cert PEM for production."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Preflight` holds a `SecretString` and an AS4 profile and is
    /// deliberately not `Debug`, so `expect_err` is unavailable here.
    fn rejection(input: &PreflightInput<'_>) -> String {
        match preflight(input) {
            Ok(_) => panic!("configuration should have been rejected"),
            Err(e) => e.to_string(),
        }
    }

    fn base<'a>() -> PreflightInput<'a> {
        PreflightInput {
            primary_mp_id: "9900001000001",
            as4_inbound_enabled: false,
            http_enabled: true,
            webdienste_enabled: false,
            webdienste_allow_unauthenticated: false,
            as4_partner: &[],
            as4_partner_cert: &[],
            as4_signing_key_pem: None,
            as4_signing_cert_pem: None,
            as4_trust_anchor_pem: None,
            as4_decryption_key_pem: None,
            as4_party_id: None,
            allow_unencrypted_as4: false,
            allow_no_as4_trust_anchor: false,
            allow_no_as4_signing: true,
            edifact_outbox_webhook_url: None,
            erp_webhook_url: None,
            netzzugang_endpoint_url: None,
            maloid_partner: &[],
            verzeichnisdienst_url: None,
            marktd_url: None,
            marktd_api_key: None,
            auth_keys: &[],
            cedar_policies: None,
            cedar_no_default_policy: false,
            oidc_issuer: None,
            oidc_audience: None,
        }
    }

    #[test]
    fn a_minimal_http_deployment_passes() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            ..base()
        };
        preflight(&input).expect("minimal config is startable");
    }

    /// `--check` must not report success for a config whose real boot bails on
    /// the missing certificate.
    #[test]
    fn a_partner_without_an_encryption_certificate_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let partners = vec!["9900001000002=https://p.example/as4".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            as4_partner: &partners,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("without encryption certificates"), "{err}");
    }

    #[test]
    fn a_plaintext_partner_endpoint_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let partners = vec!["9900001000002=http://p.example/as4".to_owned()];
        let certs = vec![format!(
            "9900001000002=-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----"
        )];
        let input = PreflightInput {
            auth_keys: &keys,
            as4_partner: &partners,
            as4_partner_cert: &certs,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("must use HTTPS"), "{err}");
    }

    #[test]
    fn an_unparseable_cedar_policy_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            cedar_policies: Some("this is not a cedar policy".to_owned()),
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("Cedar policy set is invalid"), "{err}");
    }

    #[test]
    fn an_issuer_without_an_audience_is_rejected() {
        let input = PreflightInput {
            oidc_issuer: Some("https://idp.example/realm"),
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("--oidc-audience"), "{err}");
    }

    #[test]
    fn a_plaintext_oidc_issuer_is_rejected() {
        let input = PreflightInput {
            oidc_issuer: Some("http://idp.example/realm"),
            oidc_audience: Some("api://makod"),
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("must use HTTPS"), "{err}");
    }

    #[test]
    fn no_ingest_transport_is_rejected() {
        let input = PreflightInput {
            http_enabled: false,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("No ingest transport"), "{err}");
    }

    #[test]
    fn an_authenticated_port_without_credentials_is_rejected() {
        let err = rejection(&base());
        assert!(err.contains("--auth-key"), "{err}");
    }

    /// Without signing material *and* without the webhook fallback, outbound
    /// EDIFACT never leaves the process — a silent regulatory failure that must
    /// be opted into explicitly.
    #[test]
    fn no_outbound_delivery_path_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            allow_no_as4_signing: false,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("Outbound EDIFACT delivery"), "{err}");
    }

    /// A marktd client with no credential authenticates nothing, and both
    /// gates that use it fail open — so the deployment looks gated and is not.
    #[test]
    fn marktd_without_an_api_key_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            marktd_url: Some("http://marktd:8180"),
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("no API key is configured"), "{err}");
    }

    #[test]
    fn marktd_with_an_api_key_passes() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            marktd_url: Some("http://marktd:8180"),
            marktd_api_key: Some("s3cret"),
            ..base()
        };
        preflight(&input).expect("a credentialed marktd client is startable");
    }

    /// The mirror of the missing-certificate case. A certificate whose MP-ID has
    /// no endpoint is never used, and a mistyped MP-ID presents exactly this way.
    #[test]
    fn a_partner_certificate_without_an_endpoint_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let partners = vec!["9900001000002=https://p.example/as4".to_owned()];
        let certs = vec![
            "9900001000002=-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----".to_owned(),
            // Transposed digits — no endpoint carries this MP-ID.
            "9900001000020=-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----".to_owned(),
        ];
        let input = PreflightInput {
            auth_keys: &keys,
            as4_partner: &partners,
            as4_partner_cert: &certs,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("no endpoint"), "{err}");
        assert!(err.contains("9900001000020"), "{err}");
    }

    /// Delivery destinations are parsed at startup, not at first delivery: a
    /// typo must not surface as a delivery failure on a regulated message.
    #[test]
    fn a_malformed_outbound_callback_url_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        for (label, mut input) in [
            (
                "--edifact-outbox-webhook-url",
                PreflightInput {
                    edifact_outbox_webhook_url: Some("not a url"),
                    ..base()
                },
            ),
            (
                "--erp-webhook-url",
                PreflightInput {
                    erp_webhook_url: Some("not a url"),
                    ..base()
                },
            ),
            (
                "--netzzugang-endpoint-url",
                PreflightInput {
                    netzzugang_endpoint_url: Some("not a url"),
                    ..base()
                },
            ),
        ] {
            input.auth_keys = &keys;
            let err = rejection(&input);
            assert!(err.contains(label), "expected {label} in: {err}");
        }
    }

    /// A non-HTTP scheme parses as a URL but can never be POSTed to.
    #[test]
    fn a_non_http_callback_scheme_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            erp_webhook_url: Some("file:///etc/passwd"),
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("expected an http(s) URL"), "{err}");
    }

    #[test]
    fn well_formed_callback_urls_pass() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            edifact_outbox_webhook_url: Some("http://webhook:8000/edifact"),
            erp_webhook_url: Some("https://erp.example.com/mako/events"),
            netzzugang_endpoint_url: Some("https://nzp.example.com/api"),
            ..base()
        };
        preflight(&input).expect("well-formed callback URLs are startable");
    }

    #[test]
    fn a_malformed_maloid_partner_url_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let partners = vec!["9900001000002=not a url".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            maloid_partner: &partners,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("--maloid-partner"), "{err}");
    }
}
