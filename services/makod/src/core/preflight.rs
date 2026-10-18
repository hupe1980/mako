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

use crate::as4_clients::{As4Clients, As4Setup};
use anyhow::Context as _;

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
    /// Primary Marktpartner-ID — the tenant key.
    pub primary_mp_id: &'a str,
    /// `[[party]]`, whose `[party.as4]` carry the AS4 key material.
    pub parties: &'a [crate::config::PartyConfig],
    /// The registry built from `parties` (agency per MP-ID).
    pub registry: &'a crate::party_registry::MpIdRegistry,
    /// `[as4]`.
    pub as4: &'a crate::config::As4Config,
    /// `[policy]`.
    pub policy: &'a crate::config::MakodPolicy,
    /// `--as4-addr` is set, so the inbound transport will start.
    pub as4_inbound_enabled: bool,
    /// `--http-addr` is set.
    pub http_enabled: bool,
    /// `--api-webdienste-addr` is set.
    pub webdienste_enabled: bool,
    /// `--webdienste-allow-unauthenticated`.
    pub webdienste_allow_unauthenticated: bool,
    /// Whether an mTLS client certificate **and** key are configured for
    /// calling somebody else's API-Webdienste.
    pub webdienste_client_identity: bool,
    /// `--allow-unauthenticated-webdienste-client`.
    pub allow_unauthenticated_webdienste_client: bool,
    /// Whether `--webdienste-signing-key-pem` is configured.
    pub webdienste_signing_key: bool,
    /// `--allow-no-as4-signing`.
    pub allow_no_as4_signing: bool,
    /// `--edifact-outbox-webhook-url`.
    pub edifact_outbox_webhook_url: Option<&'a str>,
    /// Whether a webhook signing secret is configured at all.
    ///
    /// A bool rather than the value: the preflight decides whether the door is
    /// credentialed, and never needs to read the credential.
    pub erp_webhook_secret_set: bool,
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
    /// `--cedar-permit-all`.
    pub cedar_permit_all: bool,
    /// `--oidc-issuer`.
    pub oidc_issuer: Option<&'a str>,
    /// `--oidc-audience`.
    pub oidc_audience: Option<&'a str>,
}

/// Validated configuration, consumed by the workers and the servers.
///
/// Producing one of these is the proof that the configuration is startable.
pub struct Preflight {
    /// One AS4 client per local MP-ID, each `As4::builder(..).build()` run —
    /// `None` when AS4 is not configured.
    pub as4: Option<As4Clients>,
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
}

/// Validate the configuration and resolve it into ready-to-use values.
///
/// # Errors
///
/// Returns the first fatal configuration error. Every error here is one that
/// would otherwise abort the real boot, which is precisely why the check runs
/// them all before `--check` reports success.
pub fn preflight(input: &PreflightInput<'_>) -> anyhow::Result<Preflight> {
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
    // Default-deny, like every other service in the platform. The permit-all
    // baseline is an explicit development opt-in, because an operator's own
    // `permit` statements cannot narrow it — beside it, a least-privilege policy
    // set grants nothing it did not already grant.
    let cedar_default_policy = if input.cedar_permit_all {
        DefaultPolicy::PermitAll
    } else {
        DefaultPolicy::Deny
    };
    // Deny with no grants authorises nothing: every route answers 403 and the
    // deployment reads as broken rather than as unconfigured.
    anyhow::ensure!(
        input.cedar_permit_all
            || input
                .cedar_policies
                .as_ref()
                .is_some_and(|p| !p.trim().is_empty()),
        "authorization is default-deny and no Cedar policies are configured, so every \
         request would be refused. Point --cedar-policy-dir (authz.cedar_policy_dir) at \
         a policy set — `services/makod/src/cedar/conservative.cedar` is the \
         least-privilege starting point — or pass --cedar-permit-all to run without \
         authorization (development only)."
    );
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

    // ── AS4 clients ──────────────────────────────────────────────────────────
    //
    // `As4::builder(Profile::bdew())…build()` per local MP-ID is the startup
    // check of the AS4 key material: every refusal is `asx::ErrorKind::Config`
    // naming the setting and the profile section — a signer that does not
    // chain to the SM-PKI root, a certificate without the MP-ID in its OU, a
    // partner endpoint without an encryption certificate, a missing decrypter,
    // a TLS key equal to a signing or encryption key. Running it here is what
    // makes `--check` report the refusal instead of the first delivery.
    let as4 = match As4Setup::from_config(
        input.as4,
        input.parties,
        input.registry,
        input.policy.as4.unbekannte_absender.get(),
    )? {
        Some(setup) => Some(As4Clients::build(setup)?),
        None => None,
    };
    anyhow::ensure!(
        !input.as4_inbound_enabled || as4.is_some(),
        "--as4-addr is set but no [[party]] has [party.as4] key material, so there is no \
         local MP-ID to serve: each MP-ID's inbox is /as4/inbox/<MP-ID>, served with its own \
         signing, encryption and TLS key pairs"
    );

    // ── API-Webdienste outbound: the identity this daemon calls under ────────
    //
    // The EDI-Energy API-Webdienste authenticate their callers by mutual TLS
    // with an EMT.API certificate from the BSI SM-PKI. Without one the TLS
    // handshake still completes locally — a client simply presents nothing —
    // and the call is refused at the counterparty. That is the worst place for
    // it to surface: the failure is a remote 4xx on a Frist rather than a line
    // of startup output, and it looks like the counterparty's outage.
    if (input.verzeichnisdienst_url.is_some() || !input.maloid_partner.is_empty())
        && !input.webdienste_client_identity
        && !input.allow_unauthenticated_webdienste_client
    {
        anyhow::bail!(
            "an API-Webdienst is configured (--verzeichnisdienst-url / --maloid-partner) but no \
             mTLS client identity is: the EDI-Energy API-Webdienste authenticate callers by \
             mutual TLS with an EMT.API certificate from the BSI SM-PKI, so every call would be \
             refused by the counterparty.\n\
             Set --webdienste-client-cert-pem and --webdienste-client-key-pem (or \
             webdienste.client_cert_pem_file / client_key_pem_file in makod.toml), or pass \
             --allow-unauthenticated-webdienste-client for dev/test."
        );
    }

    // ── API-Webdienste: every interaction is signed ──────────────────────────
    //
    // Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3: „Jede
    // API-Interaktion und der übertragene Payload wird signiert" (MUST). The
    // MaLo-ID callbacks this daemon sends — answering the `:8090` port, or
    // calling a configured partner — carry that signature only when a key is
    // configured, so a missing key is refused here rather than discovered as
    // unsigned traffic. The declared dev/test escape covers it as it covers
    // the missing mTLS identity.
    if (input.webdienste_enabled
        || !input.maloid_partner.is_empty()
        || input.verzeichnisdienst_url.is_some())
        && !input.webdienste_signing_key
        && !input.allow_unauthenticated_webdienste_client
    {
        anyhow::bail!(
            "an API-Webdienst is configured (--api-webdienste-addr / --maloid-partner / \
             --verzeichnisdienst-url) but no content signing key is: every API-Webdienste \
             interaction is signed (Regelungen zum Übertragungsweg für API-Webdienste 1.2, \
             Kap. 3.3), and a request without the signature is one the counterparty may refuse.\n\
             Set --webdienste-signing-key-pem (or webdienste.signing_key_pem_file in \
             makod.toml) to the EMT.API key, or pass --allow-unauthenticated-webdienste-client \
             for dev/test."
        );
    }

    // ── Outbound delivery path ───────────────────────────────────────────────
    //
    // Signing material drives the AS4 sender; the EDIFACT webhook is the
    // development substitute. With neither, outbound EDIFACT is logged and
    // rescheduled forever, which is a silent regulatory failure.
    anyhow::ensure!(
        as4.is_some() || input.edifact_outbox_webhook_url.is_some() || input.allow_no_as4_signing,
        "AS4 is not configured (no [party.as4] key material) and no \
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
        // An AS4 partner endpoint has to be HTTPS; this one carries the Marktlokation's postal address and its NB/MSB
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

    if input.edifact_outbox_webhook_url.is_some() {
        // The body is the rendered market message — the MaLo-IDs, the
        // counterparty and the customer data the Vorgang is about. Unsigned,
        // the receiver cannot tell a mako delivery from anything else that can
        // reach the URL, and mako cannot tell that it was read by the ERP it
        // meant. This is the AS4 substitute: the real transport authenticates
        // both ends, so the substitute has to authenticate one.
        anyhow::ensure!(
            input.erp_webhook_secret_set,
            "--edifact-outbox-webhook-url is set but no webhook signing secret is \
             configured. Outbound EDIFACT would be POSTed unsigned, carrying the \
             rendered market message and the customer data in it. Set \
             erp.webhook_secret_file in makod.toml (preferred), erp.webhook_secret, \
             or --erp-webhook-secret."
        );
    }

    if input.marktd_url.is_some() {
        // An empty key is worse than no marktd at all. The ESA consent gate and
        // the Konfigurationsprodukt guard both **fail open** on a lookup
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
        as4,
        maloid_partners,
        verzeichnisdienst_url,
        auth_keys,
        cedar_policies: input.cedar_policies.clone(),
        cedar_default_policy,
    })
}

/// Emit the non-fatal configuration warnings.
///
/// Separate from [`preflight`] because these describe a *degraded* deployment
/// rather than an unstartable one — and because `--check` should report them
/// too, which is why they are not buried in the server-start branches.
pub fn warn_on_degraded_config(input: &PreflightInput<'_>, durable_store: bool) {
    input
        .policy
        .report()
        .log("makod", time::OffsetDateTime::now_utc());
    if input.as4_inbound_enabled {
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
             transport. Set --as4-addr and [party.as4] key material for production."
        );
    }
    // Reachable only under `--allow-unauthenticated-webdienste-client`.
    if (input.webdienste_enabled
        || !input.maloid_partner.is_empty()
        || input.verzeichnisdienst_url.is_some())
        && !input.webdienste_signing_key
    {
        tracing::warn!(
            "--allow-unauthenticated-webdienste-client: no --webdienste-signing-key-pem, so \
             API-Webdienste requests leave UNSIGNED (Regelungen zum Übertragungsweg für \
             API-Webdienste 1.2, Kap. 3.3 requires a signature on every interaction). \
             Dev/test only."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Preflight` holds the AS4 clients and is deliberately not `Debug`, so
    /// `expect_err` is unavailable here.
    fn rejection(input: &PreflightInput<'_>) -> String {
        match preflight(input) {
            Ok(_) => panic!("configuration should have been rejected"),
            Err(e) => e.to_string(),
        }
    }

    /// The identity tables every fixture shares: one party, no AS4.
    struct Fixture {
        parties: Vec<crate::config::PartyConfig>,
        registry: crate::party_registry::MpIdRegistry,
        as4: crate::config::As4Config,
        policy: crate::config::MakodPolicy,
    }

    static FIXTURE: std::sync::LazyLock<Fixture> = std::sync::LazyLock::new(|| {
        let parties = vec![crate::config::PartyConfig {
            mp_id: "9900001000001".to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }];
        Fixture {
            registry: crate::party_registry::MpIdRegistry::from_config(&parties)
                .expect("valid party"),
            parties,
            as4: crate::config::As4Config::default(),
            policy: crate::config::MakodPolicy::default(),
        }
    });

    fn base<'a>() -> PreflightInput<'a> {
        let f: &'static Fixture = &FIXTURE;
        PreflightInput {
            primary_mp_id: "9900001000001",
            parties: &f.parties,
            registry: &f.registry,
            as4: &f.as4,
            policy: &f.policy,
            as4_inbound_enabled: false,
            http_enabled: true,
            webdienste_enabled: false,
            webdienste_allow_unauthenticated: false,
            webdienste_client_identity: false,
            allow_unauthenticated_webdienste_client: true,
            webdienste_signing_key: false,
            allow_no_as4_signing: true,
            edifact_outbox_webhook_url: None,
            erp_webhook_secret_set: true,
            erp_webhook_url: None,
            netzzugang_endpoint_url: None,
            maloid_partner: &[],
            verzeichnisdienst_url: None,
            marktd_url: None,
            marktd_api_key: None,
            auth_keys: &[],
            cedar_policies: None,
            // The baseline, because these fixtures are about the other doors.
            // The authorization posture has its own tests below.
            cedar_permit_all: true,
            oidc_issuer: None,
            oidc_audience: None,
        }
    }

    /// AS4 key material on disk, issued by a test SM-PKI: one local party
    /// (`9900001000001`) and one partner (`9900001000002`).
    struct As4Files {
        dir: tempfile::TempDir,
        ca: asx::testkit::pki::TestCa,
        party: asx::testkit::pki::TestParty,
        partner: asx::testkit::pki::TestParty,
    }

    impl As4Files {
        fn new() -> Self {
            let ca = asx::testkit::pki::TestCa::sm_pki();
            let party = ca.bdew_party("9900001000001");
            let partner = ca.bdew_party("9900001000002");
            let dir = tempfile::tempdir().expect("tempdir");
            let f = Self {
                dir,
                ca,
                party,
                partner,
            };
            let crl =
                f.ca.crl_signer("CCRL-S(Root)", asx::testkit::pki::KeyAlg::EcBrainpoolP256r1);
            f.write("root.pem", f.ca.cert_pem());
            f.write("ccrl-s.pem", crl.cert_pem());
            for (name, pair) in [
                ("sig", &f.party.signing),
                ("enc", &f.party.encryption),
                ("tls", &f.party.tls),
            ] {
                f.write(&format!("{name}.cert.pem"), &pair.cert_pem);
                f.write(&format!("{name}.key.pem"), &pair.key_pem);
            }
            f.write("partner.enc.pem", &f.partner.encryption.cert_pem);
            f.write("partner.sig.pem", &f.partner.signing.cert_pem);
            f
        }

        fn write(&self, name: &str, bytes: &[u8]) {
            std::fs::write(self.dir.path().join(name), bytes).expect("write");
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.dir.path().join(name)
        }

        fn party_as4(&self) -> crate::config::PartyAs4Config {
            crate::config::PartyAs4Config {
                signing_cert_pem_file: self.path("sig.cert.pem"),
                signing_key_pem_file: self.path("sig.key.pem"),
                encryption_cert_pem_file: self.path("enc.cert.pem"),
                encryption_key_pem_file: self.path("enc.key.pem"),
                tls_cert_pem_file: self.path("tls.cert.pem"),
                tls_key_pem_file: self.path("tls.key.pem"),
            }
        }

        fn as4(&self) -> crate::config::As4Config {
            crate::config::As4Config {
                addr: None,
                message_id_domain: Some("as4.test.example".to_owned()),
                trust_anchor_pem_file: Some(self.path("root.pem")),
                crl_signer_pem_file: Some(self.path("ccrl-s.pem")),
                partner: vec![crate::config::As4PartnerConfig {
                    mp_id: "9900001000002".to_owned(),
                    agency: None,
                    endpoint: Some("https://partner.example/as4/inbox/9900001000002".to_owned()),
                    encryption_cert_pem_file: Some(self.path("partner.enc.pem")),
                    signing_cert_pem_files: vec![self.path("partner.sig.pem")],
                    quirks: Vec::new(),
                }],
                allow_no_signing: false,
                lenient_receipts: None,
            }
        }

        fn parties(&self) -> Vec<crate::config::PartyConfig> {
            vec![crate::config::PartyConfig {
                mp_id: "9900001000001".to_owned(),
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
                as4: Some(self.party_as4()),
            }]
        }
    }

    /// Run the preflight over `as4` and `parties`, with AS4 inbound on.
    fn as4_preflight(
        as4: &crate::config::As4Config,
        parties: &[crate::config::PartyConfig],
    ) -> anyhow::Result<Preflight> {
        let keys = vec!["erp=token".to_owned()];
        let registry =
            crate::party_registry::MpIdRegistry::from_config(parties).expect("valid parties");
        preflight(&PreflightInput {
            auth_keys: &keys,
            parties,
            registry: &registry,
            as4,
            as4_inbound_enabled: true,
            allow_no_as4_signing: false,
            ..base()
        })
    }

    fn as4_rejection(
        as4: &crate::config::As4Config,
        parties: &[crate::config::PartyConfig],
    ) -> String {
        match as4_preflight(as4, parties) {
            Ok(_) => panic!("configuration should have been rejected"),
            Err(e) => format!("{e:#}"),
        }
    }

    /// A complete AS4 configuration builds one client per party and serves
    /// as the outbound delivery path.
    #[test]
    fn a_complete_as4_configuration_builds_its_clients() {
        let f = As4Files::new();
        let checked = as4_preflight(&f.as4(), &f.parties()).expect("startable");
        let clients = checked.as4.expect("AS4 configured");
        assert!(clients.client("9900001000001").is_some());
        assert!(clients.partners().deliverable("9900001000002").is_some());
        assert_eq!(clients.message_id("3f2c0d0e"), "3f2c0d0e@as4.test.example");
    }

    /// `--as4-addr` with no local identity to serve is refused.
    #[test]
    fn an_as4_listener_without_party_keys_is_rejected() {
        let err = as4_rejection(&crate::config::As4Config::default(), &FIXTURE.parties);
        assert!(err.contains("[party.as4]"), "{err}");
    }

    /// Every party needs its own key pairs once one has them.
    #[test]
    fn a_party_without_its_keys_beside_one_with_them_is_rejected() {
        let f = As4Files::new();
        let mut parties = f.parties();
        parties.push(crate::config::PartyConfig {
            mp_id: "9900001000003".to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
            as4: None,
        });
        let err = as4_rejection(&f.as4(), &parties);
        assert!(
            err.contains("9900001000003") && err.contains("[party.as4]"),
            "{err}"
        );
    }

    /// The trust anchor, the CRL signer and the message-id domain are
    /// required; each refusal names its key.
    #[test]
    fn the_trust_settings_are_required() {
        let f = As4Files::new();
        for (key, as4) in [
            (
                "as4.trust_anchor_pem_file",
                crate::config::As4Config {
                    trust_anchor_pem_file: None,
                    ..f.as4()
                },
            ),
            (
                "as4.crl_signer_pem_file",
                crate::config::As4Config {
                    crl_signer_pem_file: None,
                    ..f.as4()
                },
            ),
            (
                "as4.message_id_domain",
                crate::config::As4Config {
                    message_id_domain: None,
                    ..f.as4()
                },
            ),
        ] {
            let err = as4_rejection(&as4, &f.parties());
            assert!(err.contains(key), "expected {key} in: {err}");
        }
    }

    /// The trust anchor is the SM-PKI root only: a signing certificate under
    /// another CA does not build, and the asx-rs refusal reaches the operator.
    #[test]
    fn a_signer_outside_the_trust_anchor_is_rejected() {
        let f = As4Files::new();
        let other = asx::testkit::pki::TestCa::sm_pki();
        f.write("root.pem", other.cert_pem());
        let err = as4_rejection(&f.as4(), &f.parties());
        assert!(err.contains("does not build"), "{err}");
        assert!(err.contains("trust anchor"), "{err}");
    }

    /// A TLS key pair that is the signing key pair is refused (BDEW
    /// AS4-Profil 1.2 §2.3.4.4).
    #[test]
    fn a_tls_key_pair_equal_to_the_signing_key_pair_is_rejected() {
        let f = As4Files::new();
        let mut parties = f.parties();
        let keys = parties[0].as4.as_mut().expect("as4");
        keys.tls_cert_pem_file = f.path("sig.cert.pem");
        keys.tls_key_pem_file = f.path("sig.key.pem");
        let err = as4_rejection(&f.as4(), &parties);
        assert!(err.contains("same key pair"), "{err}");
    }

    /// A partner endpoint without an encryption certificate could deliver
    /// nothing (§2.2.6.2.2), and a plaintext endpoint is refused.
    #[test]
    fn an_undeliverable_partner_is_rejected() {
        let f = As4Files::new();
        let mut as4 = f.as4();
        as4.partner[0].encryption_cert_pem_file = None;
        let err = as4_rejection(&as4, &f.parties());
        assert!(err.contains("no encryption certificate"), "{err}");

        let mut as4 = f.as4();
        as4.partner[0].endpoint = Some("http://partner.example/as4".to_owned());
        let err = as4_rejection(&as4, &f.parties());
        assert!(err.contains("HTTPS"), "{err}");
    }

    /// The removed lenient-receipts switch is refused, naming the per-partner
    /// quirk that replaces it.
    #[test]
    fn the_removed_lenient_receipts_key_is_refused() {
        let f = As4Files::new();
        let as4: crate::config::As4Config = toml::from_str("lenient_receipts = true").unwrap();
        let as4 = crate::config::As4Config {
            lenient_receipts: as4.lenient_receipts,
            ..f.as4()
        };
        let err = as4_rejection(&as4, &f.parties());
        assert!(err.contains("receipt-without-non-repudiation"), "{err}");
        assert!(err.contains("[[as4.partner]] quirks"), "{err}");
    }

    /// The quirk is the per-partner replacement and is accepted.
    #[test]
    fn a_partner_quirk_is_accepted_and_an_unknown_one_refused() {
        let f = As4Files::new();
        let mut as4 = f.as4();
        as4.partner[0].quirks = vec!["receipt-without-non-repudiation".to_owned()];
        as4_preflight(&as4, &f.parties()).expect("the quirk is a valid setting");
        as4.partner[0].quirks = vec!["lenient".to_owned()];
        let err = as4_rejection(&as4, &f.parties());
        assert!(err.contains("unknown quirk"), "{err}");
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

    /// Calling an API-Webdienst without an mTLS identity is refused here, not
    /// by the counterparty.
    ///
    /// The EDI-Energy API-Webdienste authenticate callers by mutual TLS with an
    /// EMT.API certificate. Without one the handshake still completes locally —
    /// a client simply presents nothing — so the failure surfaces as a remote
    /// `4xx` on a Frist that reads like the other side's outage.
    #[test]
    fn an_api_webdienst_without_a_client_identity_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            verzeichnisdienst_url: Some("https://vz.example/api"),
            webdienste_client_identity: false,
            allow_unauthenticated_webdienste_client: false,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("mTLS client identity"), "{err}");
        assert!(
            err.contains("--webdienste-client-cert-pem"),
            "the refusal must name what to configure: {err}"
        );
    }

    /// A configured identity passes, and so does the declared dev escape.
    #[test]
    fn a_client_identity_or_the_declared_escape_passes() {
        let keys = vec!["erp=token".to_owned()];
        let with_identity = PreflightInput {
            auth_keys: &keys,
            verzeichnisdienst_url: Some("https://vz.example/api"),
            webdienste_client_identity: true,
            allow_unauthenticated_webdienste_client: false,
            webdienste_signing_key: true,
            ..base()
        };
        preflight(&with_identity).expect("a configured identity is startable");

        let declared = PreflightInput {
            auth_keys: &keys,
            verzeichnisdienst_url: Some("https://vz.example/api"),
            webdienste_client_identity: false,
            allow_unauthenticated_webdienste_client: true,
            ..base()
        };
        preflight(&declared).expect("the declared dev escape is startable");
    }

    /// Every API-Webdienste request is signed (Regelungen zum Übertragungsweg
    /// für API-Webdienste 1.2, Kap. 3.3, MUST). A daemon that would send one
    /// with no signing key is refused here rather than sending it unsigned.
    #[test]
    fn an_api_webdienst_without_a_signing_key_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let partners = vec!["9900001000001=https://nb.example/maloId".to_owned()];
        for input in [
            PreflightInput {
                auth_keys: &keys,
                webdienste_enabled: true,
                webdienste_client_identity: true,
                allow_unauthenticated_webdienste_client: false,
                webdienste_signing_key: false,
                ..base()
            },
            PreflightInput {
                auth_keys: &keys,
                maloid_partner: &partners,
                webdienste_client_identity: true,
                allow_unauthenticated_webdienste_client: false,
                webdienste_signing_key: false,
                ..base()
            },
        ] {
            let err = rejection(&input);
            assert!(err.contains("signing key"), "{err}");
            assert!(err.contains("--webdienste-signing-key-pem"), "{err}");
        }

        let signed = PreflightInput {
            auth_keys: &keys,
            maloid_partner: &partners,
            webdienste_client_identity: true,
            allow_unauthenticated_webdienste_client: false,
            webdienste_signing_key: true,
            ..base()
        };
        preflight(&signed).expect("a configured signing key is startable");
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

    /// The webhook sender is the AS4 substitute, and it carries the same
    /// payload AS4 would. AS4 authenticates both ends; an unsigned webhook
    /// authenticates neither.
    /// Authorization is default-deny, and deny with no grants authorises
    /// nothing — every route answers 403 and the deployment reads as broken.
    #[test]
    fn default_deny_without_a_policy_set_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            cedar_permit_all: false,
            cedar_policies: None,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("default-deny"), "{err}");
        assert!(
            err.contains("--cedar-policy-dir"),
            "the refusal must name what to configure: {err}"
        );
    }

    #[test]
    fn default_deny_with_a_policy_set_passes() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            cedar_permit_all: false,
            cedar_policies: Some(
                "permit(principal, action == MaKo::Action::\"IngestEdifact\", resource);"
                    .to_owned(),
            ),
            ..base()
        };
        preflight(&input).expect("a least-privilege deployment is startable");
    }

    #[test]
    fn an_edifact_webhook_without_a_signing_secret_is_rejected() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            edifact_outbox_webhook_url: Some("https://erp.example/edifact"),
            erp_webhook_secret_set: false,
            ..base()
        };
        let err = rejection(&input);
        assert!(err.contains("no webhook signing secret"), "{err}");
        assert!(
            err.contains("unsigned"),
            "the refusal must say what goes out unprotected: {err}"
        );
    }

    #[test]
    fn an_edifact_webhook_with_a_signing_secret_passes() {
        let keys = vec!["erp=token".to_owned()];
        let input = PreflightInput {
            auth_keys: &keys,
            edifact_outbox_webhook_url: Some("https://erp.example/edifact"),
            erp_webhook_secret_set: true,
            ..base()
        };
        preflight(&input).expect("a signed EDIFACT webhook is startable");
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
                    erp_webhook_secret_set: true,
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
