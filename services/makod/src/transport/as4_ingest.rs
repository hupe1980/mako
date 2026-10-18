//! AS4 inbound transport for BDEW MaKo market communication.
//!
//! One `asx::Inbound` per local MP-ID, mounted at `/as4/inbox/<MP-ID>`: each
//! certificate names one web-service address (Regelungen zum Übertragungsweg
//! für AS4 2.6 Kap. 5.2), and an inbox answers only for its own `eb:To`. A
//! request passes, in order:
//!
//! 1. **Rate limits** — per peer address and per claimed sender, before any
//!    cryptography ([`as4_rate_limit_middleware`]).
//! 2. **`asx-rs`** — signature (`X509PKIPathv1`, the signer chained to the
//!    SM-PKI root, its OU carrying the claimed MP-ID, pinned certificates for a
//!    configured partner), revocation (CRL, three-day rule), decryption — then
//!    the dedup **claim** in [`SlateDbDedup`]. A duplicate is answered with a
//!    fresh signed receipt, a message in flight with `503`; neither reaches
//!    the handler. The ebMS3 test service is receipted and never delivered.
//! 3. **[`BdewAs4IngestHandler::handle`]** — the sender policy
//!    (`[policy.as4] unbekannte_absender`), the evidence record, then the
//!    EDIFACT and Redispatch-XML legs: the same dispatch as `POST /edifact`.
//! 4. **Settlement** — `asx-rs` writes its audit record and **accepts** the
//!    claim on `Ok`, then answers with the signed receipt (NRR). On `Err` it
//!    **releases** the claim: [`Reject::permanent`] (after the message is
//!    dead-lettered) answers `EBMS:0004`, which the sender does not retry;
//!    [`Reject::new`] (nothing recorded — a store outage, a dead letter that
//!    could not be written) answers the retryable `EBMS:0202`.
//!
//! # Layering
//!
//! `mako-as4` is the BDEW profile crate (no engine dependency). This module is
//! the glue that knows both the engine (`InboxStore`, the dead-letter sink)
//! and the transport (`asx-rs`).

use std::sync::Arc;
use std::time::Duration;

use asx::as4::UserMessage;
use asx::{BoxFuture, Claim, ClaimToken, DedupStore, Reject, Verified};
use axum::Router;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_engine::ids::InboxClaimId;
use mako_engine::inbox::{InboxClaim, InboxStore};
use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::SlateDbInboxStore;

use crate::as4_clients::As4Clients;
use crate::as4_evidence::As4Evidence;
use crate::edifact_api::{EdifactApiState, MessageStatus};

/// How long the inbox waits for a request body before giving up.
pub const REQUEST_BODY_TIMEOUT: Duration = Duration::from_secs(120);

/// How long an accepted dedup key is remembered: 8 days.
///
/// `asx-rs` 0.16.2 accepts an inbound `eb:Timestamp` up to 7 days old and
/// requires a `DedupStore` to remember accepted keys for at least that long,
/// so a sender's retry — BDEW retries for 72 hours — is answered as a duplicate
/// rather than processed twice. One day above the floor for the daily purge.
pub const DEDUP_RETENTION: time::Duration = time::Duration::days(8);

// ── Dedup store ───────────────────────────────────────────────────────────────

/// [`SlateDbInboxStore`] as the `asx-rs` [`DedupStore`] of the AS4 inbox.
///
/// A claim's [`ClaimToken`] is the inbox claim id, so `accept` and `abandon`
/// are fenced by it: a holder whose lease was taken over gets `Err` and
/// changes nothing. A backend failure is `Err` too, which `asx-rs` answers
/// without a receipt — a storage outage is never "first seen".
#[derive(Clone)]
pub struct SlateDbDedup {
    store: SlateDbInboxStore,
}

impl SlateDbDedup {
    /// The AS4 dedup store over `store`.
    #[must_use]
    pub fn new(store: SlateDbInboxStore) -> Self {
        Self { store }
    }
}

/// Hand-written: the SlateDB handle behind the store is not printable.
impl std::fmt::Debug for SlateDbDedup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlateDbDedup").finish_non_exhaustive()
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The inbox claim id a token names; a token this store did not issue names
/// none, and settles nothing.
fn claim_id(token: &ClaimToken) -> Result<InboxClaimId, BoxError> {
    uuid::Uuid::parse_str(token.as_str())
        .map(InboxClaimId::from_uuid)
        .map_err(|_| "the claim token was not issued by this store".into())
}

impl DedupStore for SlateDbDedup {
    fn claim<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Claim, BoxError>> {
        Box::pin(async move {
            Ok(match self.store.claim(key).await? {
                InboxClaim::Claimed(id) => Claim::Claimed(ClaimToken::new(id.to_string())),
                InboxClaim::InFlight => Claim::InFlight,
                InboxClaim::Duplicate => Claim::Duplicate,
            })
        })
    }

    fn accept<'a>(
        &'a self,
        key: &'a str,
        token: &'a ClaimToken,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { Ok(self.store.accept(key, claim_id(token)?).await?) })
    }

    fn abandon<'a>(
        &'a self,
        key: &'a str,
        token: &'a ClaimToken,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { Ok(self.store.abandon(key, claim_id(token)?).await?) })
    }
}

// ── AS4 receive handler ───────────────────────────────────────────────────────

/// The handler of one local MP-ID's inbox.
///
/// Shared across concurrent requests via `Arc<BdewAs4IngestHandler>`.
pub struct BdewAs4IngestHandler {
    /// Shared EDIFACT dispatch state (Platform + PidRouter).
    ingest: Arc<EdifactApiState>,
    /// The local MP-ID this inbox serves (`eb:To`).
    local_mp_id: String,
    /// The clients and the partner directory the sender policy reads.
    clients: As4Clients,
    /// Where the verified evidence of every delivered message is kept.
    evidence: Option<As4Evidence>,
    /// CONTRL emitter — Empfangsbestätigung and Syntaxfehlermeldung.
    ///
    /// Per CONTRL AHB 1.0 §2.3.1 and APERAK AHB 1.1 §2.3 (Gas rules), a CONTRL
    /// Empfangsbestätigung is owed for every inbound Gas interchange except
    /// CONTRL-on-CONTRL. The Syntaxfehlermeldung is owed in **both** Sparten
    /// (§2.4: in Strom the CONTRL has no other use), which is why `startup`
    /// wires the service unconditionally rather than behind a Sparte: the
    /// service decides per interchange which of the two it owes. The AS4-level
    /// `eb:Receipt` is a separate protocol acknowledgement and satisfies
    /// neither.
    pub contrl_ack: Option<Arc<crate::contrl_ack::ContrlAckService>>,
}

impl BdewAs4IngestHandler {
    /// The handler of `local_mp_id`'s inbox.
    #[must_use]
    pub fn new(
        ingest: Arc<EdifactApiState>,
        local_mp_id: impl Into<String>,
        clients: As4Clients,
    ) -> Self {
        Self {
            ingest,
            local_mp_id: local_mp_id.into(),
            clients,
            evidence: None,
            contrl_ack: None,
        }
    }

    /// Keep the verified evidence of every delivered message.
    #[must_use]
    pub fn with_evidence(mut self, evidence: As4Evidence) -> Self {
        self.evidence = Some(evidence);
        self
    }

    /// Wire a `ContrlAckService` to emit Gas CONTRL Empfangsbestätigungen
    /// on every inbound Gas interchange (CONTRL AHB 1.0 §2.3.1).
    #[must_use]
    pub fn with_contrl_ack(mut self, svc: Arc<crate::contrl_ack::ContrlAckService>) -> Self {
        self.contrl_ack = Some(svc);
        self
    }

    /// Handle one verified, first-seen message.
    ///
    /// `Ok` lets `asx-rs` accept the claim and send the signed receipt.
    /// `Err` releases the claim: a permanent refusal after its dead letter, a
    /// transient one when nothing durable recorded the message.
    pub async fn handle(&self, msg: Verified<UserMessage>) -> Result<(), Reject> {
        let msg_id = msg.message_id().to_owned();
        let from = msg.from().to_owned();

        // ── Sender policy (`[policy.as4] unbekannte_absender`) ──────────
        // `asx-rs` has already verified that the signer chains to the SM-PKI
        // root and that its OU carries `from`. Under `nur_konfigurierte`
        // that is not enough: the sender must also be a configured partner
        // (BDEW AS4-Profil 1.2 §2.3.7 lets a receiver whitelist by the OU
        // MP-ID). Retryable on purpose — once the operator configures the
        // partner, the sender's next retry is served.
        if !self.clients.admits_sender(&from) {
            tracing::warn!(
                as4_message_id = %msg_id,
                from_party     = %from,
                to_party       = %self.local_mp_id,
                "AS4 inbound: the sender is not an [[as4.partner]] and [policy.as4] \
                 unbekannte_absender = nur_konfigurierte — refused (EBMS:0202)",
            );
            return Err(Reject::new(format!(
                "the sender {from} is not a configured partner of {}",
                self.local_mp_id
            )));
        }

        // ── Transmission-path switch (BDEW AS4-Profil 1.2 §2.3.7) ───────
        // Payload-less by definition; the receipt confirms reception. The
        // confirmSwitch answer is the operator's.
        if let Some(kind) = msg.path_switch() {
            tracing::info!(
                as4_message_id = %msg_id,
                from_party     = %from,
                kind           = ?kind,
                "AS4 inbound: transmission-path switch received",
            );
            return Ok(());
        }

        // ── Evidence ────────────────────────────────────────────────────
        // Kept before anything is dispatched: a failure here leaves no side
        // effect, so the retryable refusal reprocesses cleanly.
        if let Some(evidence) = &self.evidence {
            evidence
                .record_inbound(&self.local_mp_id, msg.evidence())
                .await?;
        }

        // One Übertragungsdatei per message — the profile defines no
        // multi-payload UserMessage.
        let payload = match msg.payloads() {
            [one] => one.bytes().clone(),
            parts => {
                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                let count = parts.len();
                self.ingest
                    .dl_sink
                    .reject(&DeadLetterReason::ProcessingError {
                        message: format!("as4_payload_count: {count} payload parts, expected one"),
                        context: AuditContext::now().with_message_ref(msg_id.clone()),
                    })
                    .await?;
                return Err(Reject::permanent(format!(
                    "{count} payload parts; one Übertragungsdatei per AS4 message"
                )));
            }
        };

        tracing::info!(
            as4_message_id = %msg_id,
            service        = %msg.service(),
            from_party     = %from,
            to_party       = %self.local_mp_id,
            payload_bytes  = payload.len(),
            "AS4 inbound: message received",
        );
        self.handle_first_seen(&msg_id, &payload).await
    }
}

/// The `asx-rs` inbox of every local MP-ID, at `/as4/inbox/<MP-ID>`.
///
/// Each route reads at most [`mako_as4::constants::MAX_MESSAGE_BYTES`] (the
/// profile limit of its client) and waits at most [`REQUEST_BODY_TIMEOUT`] for
/// the body. The dedup store is shared: the key carries the sender and the
/// message id.
pub fn router(
    clients: &As4Clients,
    dedup: SlateDbDedup,
    handler_for: impl Fn(&str) -> Arc<BdewAs4IngestHandler>,
) -> Router {
    let mut router = Router::new();
    for (mp_id, client) in clients.iter() {
        let service = tower_http::timeout::RequestBodyTimeout::new(
            inbound(client, dedup.clone(), handler_for(mp_id)),
            REQUEST_BODY_TIMEOUT,
        );
        router = router.route_service(&inbox_path(mp_id), service);
    }
    router
}

/// The `asx-rs` inbox of `client` (one local MP-ID), dispatching through
/// `handler`.
pub fn inbound(
    client: &asx::as4::As4,
    dedup: impl DedupStore,
    handler: Arc<BdewAs4IngestHandler>,
) -> asx::Inbound {
    client.inbound(dedup, move |msg: Verified<UserMessage>| {
        let handler = Arc::clone(&handler);
        async move { handler.handle(msg).await }
    })
}

/// The inbox path of local MP-ID `mp_id`.
#[must_use]
pub fn inbox_path(mp_id: &str) -> String {
    format!("/as4/inbox/{mp_id}")
}

// ── AS4 inbound rate-limit middleware ─────────────────────────────────────────

/// How many checks pass between sweeps of a keyed limiter's state.
///
/// `governor`'s keyed store grows one entry per distinct key and is pruned only
/// by an explicit `retain_recent()`; nothing calls it on a timer. Sweeping on a
/// request count rather than a clock keeps the cost proportional to traffic and
/// needs no background task to own — and so no shutdown path to get wrong.
const PRUNE_EVERY: u64 = 1024;

/// Whether this call is the one that should sweep the keyed state.
///
/// `Relaxed`: the sweep is housekeeping, so a lost or duplicated tick costs
/// nothing and the counter must not become a synchronisation point on the path
/// every inbound message takes.
fn prune_due(calls: &std::sync::atomic::AtomicU64) -> bool {
    calls
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .is_multiple_of(PRUNE_EVERY)
}

/// Admitted-check counter driving the sweep of the two inbound keyed stores.
static AS4_RATE_LIMITER_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Admitted-check counter driving the sweep of the operator keyed store.
static OPERATOR_RATE_LIMITER_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Per-peer-IP GCRA rate limiter for the AS4 inbound endpoint.
///
/// Each peer address gets its own bucket (sustained **100 requests/second**,
/// burst **50**), so one counterparty cannot exhaust the budget for everyone
/// else; an empty bucket answers `429 Too Many Requests`.
///
/// The key is the socket peer address (`ConnectInfo`). Behind a load balancer
/// that is the balancer, which must enforce its own per-client limits:
/// `X-Forwarded-For` is spoofable and not trusted here.
static AS4_RATE_LIMITER: std::sync::LazyLock<
    governor::RateLimiter<
        std::net::IpAddr,
        governor::state::keyed::DefaultKeyedStateStore<std::net::IpAddr>,
        governor::clock::DefaultClock,
    >,
> = std::sync::LazyLock::new(|| {
    use std::num::NonZeroU32;
    let quota = governor::Quota::per_second(NonZeroU32::new(100).unwrap())
        .allow_burst(NonZeroU32::new(50).unwrap());
    governor::RateLimiter::keyed(quota)
});

/// Per-sender-MP-ID GCRA rate limiter for the AS4 inbound endpoint.
///
/// **Keyed by the `eb:PartyId` inside `eb:From`**, extracted from the SOAP
/// header *before* the expensive receive pipeline (signature verification,
/// decryption) runs — that cost is exactly what a flood tries to trigger.
///
/// The value is **unverified** at this point and therefore spoofable; that is
/// acceptable for a rate limiter because both limits always apply: a spoofing
/// attacker still burns their own per-IP budget, and a spoofed partner can at
/// worst see extra `429`s (never extra capacity). Per-partner quota: 50 req/s
/// sustained, burst 25 — half the per-IP quota, still far above any real MSH's
/// peak.
///
/// **It is established immediately after**: `asx-rs` refuses a signer whose
/// SM-PKI certificate does not carry the claimed MP-ID in its OU, or, for a
/// configured partner, is not one of its pinned certificates — so a sender
/// claiming to be someone else fails verification.
static AS4_SENDER_RATE_LIMITER: std::sync::LazyLock<
    governor::RateLimiter<
        String,
        governor::state::keyed::DefaultKeyedStateStore<String>,
        governor::clock::DefaultClock,
    >,
> = std::sync::LazyLock::new(|| {
    use std::num::NonZeroU32;
    let quota = governor::Quota::per_second(NonZeroU32::new(50).unwrap())
        .allow_burst(NonZeroU32::new(25).unwrap());
    governor::RateLimiter::keyed(quota)
});

/// Extract the first `eb:From/eb:PartyId` text from a SOAP prefix.
///
/// A deliberately cheap scan over the first bytes of the (possibly MIME-
/// wrapped) request body — no XML parse, no allocation beyond the result.
/// Returns `None` when the structure is absent; the caller then applies only
/// the per-IP limit.
fn extract_sender_mp_id(body: &[u8]) -> Option<String> {
    // The ebMS header sits early in the envelope; 16 KiB is generous.
    let window = &body[..body.len().min(16 * 1024)];
    let text = std::str::from_utf8(window).ok()?;
    let from_idx = text.find(":From>").or_else(|| text.find(":From "))?;
    let after_from = &text[from_idx..];
    let pid_open = after_from.find(":PartyId")?;
    let after_open = &after_from[pid_open..];
    let gt = after_open.find('>')?;
    let rest = &after_open[gt + 1..];
    let lt = rest.find('<')?;
    let value = rest[..lt].trim();
    // MP-IDs are 13-digit codes or 16-char EIC — reject anything else so a
    // crafted header cannot grow the keyed state with garbage keys.
    let valid = (value.len() == 13 && value.bytes().all(|b| b.is_ascii_digit()))
        || (value.len() == 16
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    valid.then(|| value.to_owned())
}

/// Axum middleware combining the per-peer and per-sender AS4 rate limits.
///
/// Order: per-IP first (no body read), then per-MP-ID on the buffered body.
/// The body is buffered once here and handed onward — the AS4 router buffers
/// it anyway, so this adds no extra copy of the payload bytes.
pub async fn as4_rate_limit_middleware(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Orchestrator probes are exempt: they are cheap, unauthenticated by
    // design, and a 429 on a liveness probe reads as a dead container.
    if crate::health::is_health_path(req.uri().path()) {
        return next.run(req).await;
    }
    // `governor` prunes its keyed store only when asked, and an inbound AS4
    // endpoint is reachable by anyone holding a BDEW/DVGW certificate: without
    // the sweep the map keeps one permanent entry per peer address and per
    // presented sender id, for the process lifetime.
    if prune_due(&AS4_RATE_LIMITER_CALLS) {
        AS4_RATE_LIMITER.retain_recent();
        AS4_SENDER_RATE_LIMITER.retain_recent();
    }
    if AS4_RATE_LIMITER.check_key(&peer.ip()).is_err() {
        tracing::warn!(
            peer = %peer.ip(),
            "AS4 inbound rate limit exceeded (100 req/s per peer) — returning 429",
        );
        return too_many_requests();
    }

    // Buffer the body to inspect the ebMS sender. 64 MiB cap matches the
    // AS4 router's own body limit.
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return too_many_requests_status(axum::http::StatusCode::PAYLOAD_TOO_LARGE),
    };
    if let Some(sender) = extract_sender_mp_id(&bytes)
        && AS4_SENDER_RATE_LIMITER.check_key(&sender).is_err()
    {
        tracing::warn!(
                peer      = %peer.ip(),
                sender_mp_id = %sender,
                "AS4 inbound per-sender rate limit exceeded (50 req/s) — returning 429. \
                 Sender identity is pre-verification (spoofable); the per-IP limit \
                 has already been applied.",
        );
        return too_many_requests();
    }
    let req = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    next.run(req).await
}

fn too_many_requests() -> axum::response::Response {
    too_many_requests_status(axum::http::StatusCode::TOO_MANY_REQUESTS)
}

fn too_many_requests_status(status: axum::http::StatusCode) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("Retry-After", "1")
        .header("Content-Type", "text/plain")
        .body(axum::body::Body::from(
            "Inbound rate limit exceeded. Retry after 1 second.",
        ))
        .unwrap_or_else(|_| {
            axum::response::Response::builder()
                .status(status)
                .body(axum::body::Body::empty())
                .unwrap()
        })
}

/// Per-peer-IP GCRA rate limiter for the operator-facing ports.
///
/// **Deliberately not [`AS4_RATE_LIMITER`].** Behind a proxy — the documented
/// deployment — every client of every port arrives from the ingress address,
/// so a shared bucket collapses all three ports into one 100 req/s budget, and
/// an ERP batch on `:8080` could `429` a trading partner's AS4 delivery, whose
/// retry schedule is what stands between us and a missed Frist. The AS4 budget
/// is reserved for counterparties.
static OPERATOR_RATE_LIMITER: std::sync::LazyLock<
    governor::RateLimiter<
        std::net::IpAddr,
        governor::state::keyed::DefaultKeyedStateStore<std::net::IpAddr>,
        governor::clock::DefaultClock,
    >,
> = std::sync::LazyLock::new(|| {
    use std::num::NonZeroU32;
    let quota = governor::Quota::per_second(NonZeroU32::new(100).unwrap())
        .allow_burst(NonZeroU32::new(50).unwrap());
    governor::RateLimiter::keyed(quota)
});

/// Axum middleware enforcing the per-peer inbound rate limit alone.
///
/// Used by the REST API (`:8080`) and the API-Webdienste Strom port (`:8090`).
/// Neither carries an ebMS envelope to read a sender identity from, so the
/// per-sender limiter does not apply. Both draw on the operator-facing
/// bucket rather than the AS4 port's, so an ERP batch cannot spend a trading
/// partner's delivery budget.
///
/// Returns `429 Too Many Requests` with a `Retry-After: 1` header when the
/// peer's GCRA token bucket is exhausted, and logs each rejection so operators
/// can detect unusual traffic patterns.
pub async fn rate_limit_middleware(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // See `as4_rate_limit_middleware`: health probes are never throttled.
    if crate::health::is_health_path(req.uri().path()) {
        return next.run(req).await;
    }
    if prune_due(&OPERATOR_RATE_LIMITER_CALLS) {
        OPERATOR_RATE_LIMITER.retain_recent();
    }
    if OPERATOR_RATE_LIMITER.check_key(&peer.ip()).is_err() {
        tracing::warn!(
            method = %req.method(),
            uri    = %req.uri(),
            peer   = %peer.ip(),
            "inbound rate limit exceeded (100 req/s per peer) — returning 429. \
             Possible misconfigured or malicious counterparty."
        );
        return too_many_requests();
    }
    next.run(req).await
}

/// Whether a DVGW interchange carried a GABi-Gas ALOCAT.
///
/// CONTRL AHB 1.0 §2.3.1 shortens that interchange's CONTRL window to 45
/// minutes; every other DVGW format keeps the 6 hours.
#[cfg(test)]
mod sender_extract_tests {
    use super::extract_sender_mp_id;

    /// The happy path: a BDEW ebMS header yields the 13-digit sender MP-ID.
    #[test]
    fn extracts_thirteen_digit_sender() {
        let soap = br#"<S12:Envelope><S12:Header><eb:Messaging>
            <eb:UserMessage><eb:PartyInfo>
            <eb:From><eb:PartyId type="urn:oasis:names:tc:ebcore:partyid-type:unregistered:BDEW">9900001000001</eb:PartyId>
            <eb:Role>ZSH</eb:Role></eb:From>
            <eb:To><eb:PartyId>9900001000002</eb:PartyId></eb:To>
            </eb:PartyInfo></eb:UserMessage></eb:Messaging></S12:Header></S12:Envelope>"#;
        assert_eq!(extract_sender_mp_id(soap).as_deref(), Some("9900001000001"));
    }

    /// Garbage values must not become limiter keys — an attacker could
    /// otherwise grow the keyed state unboundedly.
    #[test]
    fn rejects_invalid_shapes() {
        let bad = br#"<eb:From><eb:PartyId>DROP TABLE x; --</eb:PartyId></eb:From>"#;
        assert_eq!(extract_sender_mp_id(bad), None);
        assert_eq!(extract_sender_mp_id(b"no xml at all"), None);
        assert_eq!(extract_sender_mp_id(&[0xFF, 0xFE]), None);
    }

    /// A 16-char EIC is accepted.
    #[test]
    fn accepts_eic() {
        let soap = br#"<eb:From><eb:PartyId>10XDE-EON-NETZ-C</eb:PartyId></eb:From>"#;
        assert_eq!(
            extract_sender_mp_id(soap).as_deref(),
            Some("10XDE-EON-NETZ-C")
        );
    }
}

#[cfg(test)]
mod dispatch_failure_tests {
    use std::sync::Mutex;

    use mako_engine::dead_letter::{DeadLetterReason, DeadLetterSink};

    use crate::edifact_api::dead_letter_dispatch_failure;

    #[derive(Default)]
    struct CapturingSink(Mutex<Vec<(String, String)>>);

    impl DeadLetterSink for CapturingSink {
        fn reject<'a>(
            &'a self,
            reason: &'a DeadLetterReason,
        ) -> mako_engine::dead_letter::RejectFuture<'a> {
            let detail = match reason {
                DeadLetterReason::ProcessingError { message, .. } => message.clone(),
                other => format!("{other:?}"),
            };
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((reason.label().to_owned(), detail));
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// A dispatch failure must leave a dead letter naming it. Logged as
    /// "non-fatal" while the receipt still goes out and the dedup entry stays
    /// durable, the message is lost with no § 147 AO trace.
    #[tokio::test]
    async fn a_failed_dispatch_is_dead_lettered() {
        let sink = CapturingSink::default();
        dead_letter_dispatch_failure(
            &sink,
            mako_engine::dead_letter::AuditContext::now()
                .with_message_type("UTILMD")
                .with_pid(mako_engine::ids::Pid::new(55001))
                .with_message_ref("as4-msg-1"),
            "gpke-supplier-change",
            &mako_engine::error::EngineError::Registry {
                message: "store unavailable".to_owned(),
                transient: false,
            },
        )
        .await
        .expect("the capturing sink records");
        let recorded = sink.0.lock().expect("not poisoned");
        assert_eq!(recorded.len(), 1, "exactly one dead letter");
        let (label, detail) = &recorded[0];
        assert_eq!(label, "processing_error");
        assert!(
            detail.starts_with("dispatch_failed: workflow gpke-supplier-change:"),
            "the reason must identify the failing dispatch: {detail}"
        );
        assert!(
            detail.contains("store unavailable"),
            "the underlying error must survive into the audit trail: {detail}"
        );
    }
}

impl BdewAs4IngestHandler {
    /// Dispatch the Übertragungsdatei of a verified, first-seen message.
    ///
    /// **Every refusal here dead-letters the message before it returns**, with
    /// the write awaited, and then refuses for good ([`Reject::permanent`],
    /// `EBMS:0004`): the sender does not retransmit a message that is already
    /// recorded and could never pass. A dead letter that cannot be written is
    /// the `?` on its `await` — a retryable [`Reject::new`], so the message
    /// comes round again rather than vanishing. `check-as4-controls` keeps
    /// every permanent refusal behind an awaited dead letter.
    async fn handle_first_seen(&self, msg_id: &str, edifact: &[u8]) -> Result<(), Reject> {
        let msg_id = msg_id.to_owned();
        // The ÜZ: the instant this door took the message, which the receipt
        // returned on this same request confirms. Every arrival-anchored Frist
        // of the messages inside runs from it, however long dispatch takes.
        let received_at = time::OffsetDateTime::now_utc();

        // ── Redispatch 2.0 XML leg ────────────────────────────────────
        // The BDEW AS4 channel carries two payload formats: EDIFACT
        // interchanges and the nine Redispatch 2.0 XML document types
        // (BK6-20-059/-061).  XML payloads never enter the EDIFACT
        // pipeline — no UNB envelope, no test indicator, no CONTRL
        // obligation.
        if crate::redispatch_xml_ingest::looks_like_xml(edifact) {
            let Some(dispatcher) = self.ingest.dispatcher.as_deref() else {
                // A deployment fault, not a sender fault: retransmitting
                // reaches the same unconfigured daemon, and the dedup key
                // is already spent, so the retry is answered 200 and the
                // document is gone. The dead letter is the only record.
                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                self.ingest
                    .dl_sink
                    .reject(&DeadLetterReason::ProcessingError {
                        message: "redispatch_xml_no_dispatcher_configured".to_owned(),
                        context: AuditContext::now()
                            .with_message_type("REDISPATCH-XML")
                            .with_message_ref(msg_id.clone()),
                    })
                    .await?;
                tracing::error!(
                    as4_message_id = %msg_id,
                    "AS4 ingest: XML payload received but no Phase 2 dispatcher \
                     is wired — dead-lettered",
                );
                return Err(Reject::permanent(
                    "XML payload received but workflow dispatch is not configured",
                ));
            };
            return match crate::redispatch_xml_ingest::dispatch_redispatch_xml(dispatcher, edifact)
                .await
            {
                Ok(crate::ingest_dispatcher::IngestOutcome::Skipped {
                    workflow_name,
                    reason,
                }) => {
                    // Parse/validation failure or unroutable document —
                    // reject *without* a receipt: an AS4 receipt asserts
                    // successful reception, and the sender must correct
                    // and retransmit.
                    // The sender has to correct and resend under a new
                    // `eb:MessageId`; this one's dedup key is spent, so a
                    // plain retransmission is answered 200 and vanishes.
                    // A regulated document arrived and was refused — that
                    // is a record we owe regardless of whose fault it is.
                    use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                    self.ingest
                        .dl_sink
                        .reject(&DeadLetterReason::ProcessingError {
                            message: format!("redispatch_xml_rejected: {workflow_name}: {reason}"),
                            context: AuditContext::now()
                                .with_message_type("REDISPATCH-XML")
                                .with_message_ref(msg_id.clone()),
                        })
                        .await?;
                    tracing::warn!(
                        as4_message_id = %msg_id,
                        workflow       = %workflow_name,
                        reason         = %reason,
                        "AS4 ingest: Redispatch XML payload rejected — dead-lettered",
                    );
                    Err(Reject::permanent(format!(
                        "Redispatch XML rejected: {reason}"
                    )))
                }
                Ok(outcome) => {
                    tracing::info!(
                        as4_message_id = %msg_id,
                        outcome        = ?outcome,
                        "AS4 ingest: Redispatch XML document dispatched",
                    );
                    Ok(())
                }
                Err(e) => {
                    // No durable business record was written, and the
                    // dedup key is already spent: the `ReceptionAwareness`
                    // retry carries the same `eb:MessageId` and is
                    // answered 200 without dispatching. Without this dead
                    // letter the document is lost while the counterparty
                    // holds an acknowledgement saying it arrived.
                    use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                    self.ingest
                        .dl_sink
                        .reject(&DeadLetterReason::ProcessingError {
                            message: format!("redispatch_xml_dispatch_failed: {e}"),
                            context: AuditContext::now()
                                .with_message_type("REDISPATCH-XML")
                                .with_message_ref(msg_id.clone()),
                        })
                        .await?;
                    tracing::error!(
                        as4_message_id = %msg_id,
                        error          = %e,
                        "AS4 ingest: Redispatch XML dispatch failed — dead-lettered",
                    );
                    Err(Reject::permanent(format!(
                        "Redispatch XML dispatch failed: {e}"
                    )))
                }
            };
        }

        // The `UNB` alone, not a full parse. A full parse errs on a
        // § 2.13 party mismatch, a UNZ count mismatch, too many messages,
        // or any single unparseable message — and the guards below would
        // then be skipped on exactly the interchange least worth
        // trusting, letting a test-flagged or misaddressed one reach
        // production workflows because one message inside it was
        // malformed.
        let unb = self.ingest.platform.parse_interchange_header(edifact).ok();

        // ── Recipient guard (Allgemeine Festlegungen V6.1d § 2.13) ────
        //
        // `UNB` DE0010 names who the interchange is addressed to. An
        // authenticated counterparty is still not entitled to hand us an
        // interchange addressed to a third party: routing it would apply
        // another market participant's message to our own processes, and
        // the answer would go back under our MP-ID.
        //
        // Separate from the sender binding above, and not implied by it.
        // That one establishes *who sent this*; this one establishes
        // *that it was sent to us*. `is_own_mp_id` covers every MP-ID
        // this deployment holds, so a combined-role instance addressed
        // under its NB or its MSB identity both pass.
        if let Some(header) = unb.as_ref()
            && !self.ingest.mp_id_registry.is_own_mp_id(&header.receiver_id)
        {
            use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
            self.ingest
                .dl_sink
                .reject(&DeadLetterReason::ProcessingError {
                    message: format!(
                        "interchange addressed to a foreign MP-ID: UNB DE0010 = {}",
                        header.receiver_id
                    ),
                    context: AuditContext::from_interchange(
                        &header.sender_id,
                        &header.receiver_id,
                        &header.control_ref,
                    ),
                })
                .await?;
            tracing::warn!(
                as4_message_id = %msg_id,
                sender = %header.sender_id,
                receiver = %header.receiver_id,
                control_ref = %header.control_ref,
                "AS4 ingest: interchange is addressed to an MP-ID this deployment \
                 does not hold — refused",
            );
            return Err(Reject::permanent(
                "UNB DE0010 names an MP-ID this deployment does not hold",
            ));
        }

        // ── Test-indicator guard (§AF §3 / Allgemeine Festlegungen V6.1d §3) ──
        // Reject before dispatching any messages.
        if let Some(header) = unb.as_ref()
            && header.test_indicator
        {
            use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
            let ctx = AuditContext::from_interchange(
                &header.sender_id,
                &header.receiver_id,
                &header.control_ref,
            );
            self.ingest
                .dl_sink
                .reject(&DeadLetterReason::TestMessage { context: ctx })
                .await?;
            tracing::warn!(
                as4_message_id = %msg_id,
                sender = %header.sender_id,
                receiver = %header.receiver_id,
                control_ref = %header.control_ref,
                "AS4 ingest: test interchange (DE0035=1) rejected — \
                 must not process test messages on production endpoint (§AF §3)",
            );
            // The sender is owed the reason. `UNB` DE 0035 is a value the
            // production endpoint does not support, which is exactly
            // `UCI` DE 0085 = 25 „Test-Kennzeichen nicht unterstützt"
            // (CONTRL AHB 1.0 Kap. 3).
            if let Some(contrl_svc) = self.contrl_ack.as_deref()
                && let Err(e) = contrl_svc
                    .emit_syntax_error(
                        edifact,
                        &header.control_ref,
                        &header.receiver_id,
                        &header.sender_id,
                        crate::contrl_ack::SyntaxFehler::TestKennzeichen,
                    )
                    .await
            {
                self.ingest
                    .dl_sink
                    .reject(&DeadLetterReason::ProcessingError {
                        message: format!("contrl_syntaxfehler_failed: {e}"),
                        context: AuditContext::from_interchange(
                            &header.sender_id,
                            &header.receiver_id,
                            &header.control_ref,
                        )
                        .with_message_type("CONTRL"),
                    })
                    .await?;
            }
            return Err(Reject::permanent(
                "test interchange rejected: DE0035=1 on production endpoint",
            ));
        }
        // ── DVGW gas transport ────────────────────────────────────────
        // Tried first, because a DVGW message rides `ORDERS`/`ORDRSP` and
        // the BDEW parser would accept it as one — yielding a
        // Prüfidentifikator from the wrong catalogue. `try_ingest` sniffs
        // `BGM` DE 1001 and returns `None` for a BDEW interchange, which
        // costs that path only the sniff.
        {
            if let Some(report) =
                crate::dvgw_ingest::try_ingest(self.ingest.as_ref(), edifact).await
            {
                // A refused DVGW message without its dead letter must not be
                // settled: the claim is abandoned and the sender retransmits.
                if let Some(e) = report.unrecorded {
                    return Err(Reject::new(mako_engine::error::EngineError::dead_letter(e)));
                }
                tracing::info!(
                    as4_message_id = %msg_id,
                    accepted = report.accepted(),
                    rejected = report.rejected(),
                    "AS4 ingest: DVGW interchange dispatched",
                );
                // The EDIFACT-level CONTRL Empfangsbestätigung (CONTRL
                // AHB 1.0 §2.3.1) is owed within six wall-clock hours for
                // every inbound *Gas* interchange, and a DVGW interchange
                // is Gas by definition. The AS4 `eb:Receipt` below is a
                // *protocol* acknowledgement and does not discharge it.
                if let Some(contrl_svc) = self.contrl_ack.as_deref() {
                    let sender = report.sender_mp_id.clone().unwrap_or_default();
                    if let Err(e) = contrl_svc
                        .emit_for_dvgw_interchange(
                            &sender,
                            &report.interchange_ref,
                            &report.recipient_mp_id,
                            super::contrl_ack::dvgw_report_has_alocat(&report),
                        )
                        .await
                    {
                        use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                        self.ingest
                            .dl_sink
                            .reject(&DeadLetterReason::ProcessingError {
                                message: format!("contrl_ack_failed: {e}"),
                                context: AuditContext::now()
                                    .with_message_type("CONTRL")
                                    .with_receiver_eic(report.recipient_mp_id.as_str())
                                    .with_message_ref(report.interchange_ref.as_str()),
                            })
                            .await?;
                    }
                }

                // The AS4 receipt is owed regardless of which family the
                // payload belongs to, so the DVGW path returns through the
                // same closure the BDEW path ends with rather than
                // short-circuiting it.
                return Ok(());
            }
        }

        // ── EDIFACT dispatch ──────────────────────────────────────────
        // Collect parsed messages so they can be passed to ContrlAckService
        // after the dispatch loop (CONTRL AHB 1.0 §2.3.1 Gas obligation).
        // The recipient MP-ID (UNB DE0010) drives Sparte detection and the
        // CONTRL sender MP-ID.
        let (interchange_ref, recipient_mp_id): (String, String) =
            if let Ok(pi) = self.ingest.platform.parse_interchange_full(edifact) {
                (
                    pi.header.control_ref.to_string(),
                    pi.header.receiver_id.to_string(),
                )
            } else {
                (msg_id.clone(), String::new())
            };
        // ── Syntax check of the whole Übertragungsdatei first ─────────
        //
        // CONTRL AHB 1.0 Kap. 1.3: one CONTRL per Übertragungsdatei, which
        // „entweder … bestätigt oder die gesamte Übertragungsdatei
        // zurückgewiesen" — and Kap. 1.2: on a syntax error „alle in der
        // Übertragungsdatei enthaltenen Geschäftsvorfälle [werden] vom
        // Empfänger nicht verarbeitet". So the file is parsed in full before
        // any message is dispatched, and a file with a broken message beside
        // readable ones dispatches none of them.
        let results: Vec<Result<AnyMessage, edi_energy::Error>> = self
            .ingest
            .platform
            .parse_interchange(std::io::Cursor::new(edifact))
            .collect();
        let broken = results.iter().filter(|r| r.is_err()).count();
        if broken > 0 && broken < results.len() {
            crate::edifact_api::reject_broken_interchange(
                self.ingest.as_ref(),
                self.contrl_ack.as_deref(),
                edifact,
                &results,
                &msg_id,
            )
            .await?;
            // The file arrived; the CONTRL, not the transport, says it is
            // rejected.
            return Ok(());
        }
        let mut accepted = 0usize;
        let mut rejected = 0usize;
        let mut parsed_msgs: Vec<edi_energy::AnyMessage> = Vec::new();
        // `UCI` DE 0085 of the Syntaxfehlermeldung, if one is owed. The
        // segment carries one code, so it is the first fault that names
        // the file (§5.3.3 reports at the lowest level that can express
        // a fault, and an interchange-level UCI has no lower level here).
        let mut first_syntax_error = crate::contrl_ack::SyntaxFehler::UngueltigerWert;
        let mut saw_syntax_error = false;
        for result in results {
            match result {
                Err(e) => {
                    rejected += 1;
                    if !saw_syntax_error {
                        first_syntax_error = crate::contrl_ack::SyntaxFehler::from_parse_error(&e);
                        saw_syntax_error = true;
                    }
                    // Deliberately *not* counted as a validation failure.
                    // `makod_validation_failed_total` carries a message type
                    // and a release, and a message that did not parse has
                    // neither. Counting it under fixed labels such as
                    // `("edifact", "parse_error")` would make the metric
                    // report a message type that does not exist and bury
                    // the AHB failures it is for. The dead letter below is
                    // the record, and it alerts as
                    // `makod_dead_letter_recorded_total{reason="processing_error"}`.
                    //
                    // § 147 AO / GoBD: a message that fails to parse inside an
                    // otherwise-accepted interchange must leave a durable
                    // trace — the AS4 receipt confirms receipt of the whole
                    // interchange, so a metric + log alone would make the
                    // failed message vanish from the audit trail.
                    {
                        use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                        self.ingest
                            .dl_sink
                            .reject(&DeadLetterReason::ProcessingError {
                                message: format!("EDIFACT parse error: {e}"),
                                context: AuditContext::now()
                                    .with_message_type("UNPARSEABLE")
                                    .with_message_ref(msg_id.as_str()),
                            })
                            .await?;
                    }
                    tracing::warn!(
                        as4_message_id = %msg_id,
                        error          = %e,
                        "AS4 ingest: EDIFACT parse error — dead-lettered",
                    );
                }
                Ok(msg) => {
                    let message_type = msg.try_message_type().map(|t| t.to_string());
                    let pid = msg
                        .detect_pruefidentifikator()
                        .ok()
                        .and_then(|p| mako_engine::ids::Pid::from_u32(p.as_u32()));
                    let workflow = pid
                        .and_then(|p| self.ingest.resolve_workflow(p.as_u32(), &recipient_mp_id))
                        .map(str::to_owned);

                    // Same classifier as the REST door — the two used
                    // to decide independently, and a PID-less non-CONTRL
                    // message was `NoPid` (accepted, unrecorded) here
                    // and `MissingPid` (dead-lettered) there.
                    let status = MessageStatus::classify(&msg, pid, workflow.as_deref());

                    // Conformance is recorded for every routed message,
                    // not only the ones whose adapter happens to ask.
                    if status == MessageStatus::Routed {
                        crate::edifact_api::record_ahb_conformance(&msg);
                    }

                    // Dead-letter unroutable messages (§ 147 AO / GoBD).
                    if status.is_unroutable() {
                        use mako_engine::dead_letter::AuditContext;
                        let ctx = AuditContext::now()
                            .with_message_type(message_type.as_deref().unwrap_or(""))
                            .with_message_ref(msg_id.as_str())
                            .with_receiver_eic(recipient_mp_id.as_str());
                        let ctx = if let Some(p) = pid {
                            ctx.with_pid(p)
                        } else {
                            ctx
                        };
                        let dead_pid = pid.unwrap_or(mako_engine::ids::Pid::new(1));
                        let (reason, result) =
                            crate::edifact_api::unroutable_rejection(status, &msg, dead_pid, ctx);
                        EngineMetrics::global().inbound_received(dead_pid.as_u32(), result);
                        self.ingest.dl_sink.reject(&reason).await?;
                    }

                    tracing::info!(
                        as4_message_id = %msg_id,
                        message_type   = ?message_type,
                        pid            = pid.map(|p| p.as_u32()),
                        workflow       = ?workflow,
                        status         = ?status,
                        "AS4 ingest: EDIFACT message dispatched",
                    );

                    // Phase 2: execute workflow command if dispatcher is wired.
                    if let (Some(pid_val), Some(wf_name)) = (pid, workflow.as_deref())
                        && let Some(dispatcher) = self.ingest.dispatcher.as_deref()
                    {
                        match dispatcher
                            .dispatch(&msg, wf_name, pid_val.as_u32(), received_at)
                            .await
                        {
                            Ok(outcome) => {
                                // The receipt has already gone out, so a
                                // message the router claimed and no arm
                                // consumed is acknowledged and lost —
                                // recorded, not merely logged
                                // (§ 147 AO / GoBD).
                                if let Some((wf, reason)) = outcome.coverage_gap() {
                                    use mako_engine::dead_letter::{
                                        AuditContext, DeadLetterReason,
                                    };
                                    self.ingest
                                        .dl_sink
                                        .reject(&DeadLetterReason::NotDispatchable {
                                            workflow_name: wf.to_owned(),
                                            pid: pid_val,
                                            reason: reason.to_owned(),
                                            context: AuditContext::now()
                                                .with_message_type(
                                                    message_type.as_deref().unwrap_or(""),
                                                )
                                                .with_message_ref(msg_id.as_str())
                                                .with_receiver_eic(recipient_mp_id.as_str())
                                                .with_pid(pid_val),
                                        })
                                        .await?;
                                }
                                tracing::debug!(
                                    as4_message_id = %msg_id,
                                    workflow       = %wf_name,
                                    outcome        = ?outcome,
                                    "AS4 ingest: Phase 2 command dispatched",
                                );
                            }
                            Err(e) => {
                                crate::edifact_api::dead_letter_dispatch_failure(
                                    self.ingest.dl_sink.as_ref(),
                                    mako_engine::dead_letter::AuditContext::now()
                                        .with_message_type(message_type.as_deref().unwrap_or(""))
                                        .with_pid(pid_val)
                                        .with_message_ref(&msg_id),
                                    wf_name,
                                    &e,
                                )
                                .await?;
                                tracing::error!(
                                    as4_message_id = %msg_id,
                                    workflow       = %wf_name,
                                    error          = %e,
                                    "AS4 ingest: Phase 2 command dispatch failed — \
                                     dead-lettered",
                                );
                            }
                        }
                    }

                    accepted += 1;
                    // Collect for CONTRL Empfangsbestätigung (Gas interchanges).
                    parsed_msgs.push(msg);
                }
            }
        }

        if accepted == 0 && rejected > 0 {
            // Not one message in the Übertragungsdatei could be read, so
            // the file is not processed further — which is exactly what
            // a Syntaxfehlermeldung states (CONTRL AHB 1.0 §2.3.2 /
            // §2.4.2). It is owed in **both** Sparten: §2.4 uses the
            // CONTRL in Strom for nothing else. The `UNB` parsed, or
            // `interchange_ref` would be the AS4 message id — §2.2.2.1
            // makes a CONTRL impossible in that case and the dead letter
            // above is the record instead.
            if let Some(contrl_svc) = self.contrl_ack.as_deref()
                && let Ok(pi) = self.ingest.platform.parse_interchange_full(edifact)
                && let Err(e) = contrl_svc
                    .emit_syntax_error(
                        edifact,
                        &pi.header.control_ref,
                        &pi.header.receiver_id,
                        &pi.header.sender_id,
                        first_syntax_error,
                    )
                    .await
            {
                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                self.ingest
                    .dl_sink
                    .reject(&DeadLetterReason::ProcessingError {
                        message: format!("contrl_syntaxfehler_failed: {e}"),
                        context: AuditContext::now()
                            .with_message_type("CONTRL")
                            .with_receiver_eic(recipient_mp_id.as_str())
                            .with_message_ref(interchange_ref.as_str()),
                    })
                    .await?;
            }
            return Err(Reject::permanent(
                "AS4 payload contained no valid EDIFACT messages",
            ));
        }

        // ── Gas CONTRL Empfangsbestätigung ────────────────────────────
        // CONTRL AHB 1.0 §2.3.1: for every inbound Gas interchange
        // (except CONTRL-on-CONTRL) the receiver must send a CONTRL
        // Empfangsbestätigung within 6 wall-clock hours.
        // The AS4 eb:Receipt above is a *protocol* acknowledgement and
        // does not satisfy this EDIFACT-level obligation.
        if let Some(contrl_svc) = self.contrl_ack.as_deref() {
            let refs: Vec<&AnyMessage> = parsed_msgs.iter().collect();
            if let Err(e) = contrl_svc
                .emit_for_interchange(&refs, &interchange_ref, &recipient_mp_id)
                .await
            {
                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                self.ingest
                    .dl_sink
                    .reject(&DeadLetterReason::ProcessingError {
                        message: format!("contrl_ack_failed: {e}"),
                        context: AuditContext::now()
                            .with_message_type("CONTRL")
                            .with_receiver_eic(recipient_mp_id.as_str())
                            .with_message_ref(interchange_ref.as_str()),
                    })
                    .await?;
            }
        }

        // ── Synchronous receipt (BDEW AS4-Profil §2.2.4) ──────────────
        Ok(())
    }
}
