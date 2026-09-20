//! AS4 inbound transport for BDEW MaKo market communication.
//!
//! This module wires the [`asx_rs`] AS4 receive pipeline to the same
//! EDIFACT dispatch layer used by the REST ingest API.  Every EDIFACT
//! UserMessage delivered over AS4 passes through:
//!
//! 1. **HTTP pre-validation** — performed by the `asx_rs` Axum router before
//!    our handler is invoked (method, `Content-Type: application/soap+xml`,
//!    body size / read timeout).
//! 2. **AS4 receive pipeline** — WS-Security signature verification, replay
//!    deduplication via [`SlateDbDedupBridge`], MIME multipart parsing, payload
//!    extraction (`receive_push_with_dedup_async`).
//! 3. **EDIFACT dispatch** — identical to the REST `POST /edifact` path:
//!    `Platform::parse_interchange` → `PidRouter` → structured log.
//! 4. **Synchronous receipt** — an `eb:Receipt` SignalMessage is returned on
//!    the same HTTP connection, satisfying the BDEW AS4 MEP requirement.
//!    When receipt-signing credentials are configured
//!    ([`BdewAs4IngestHandler::with_receipt_credentials`]), the receipt is
//!    **signed** and echoes the inbound message's `ds:Reference` digests as
//!    NonRepudiationInformation, satisfying Non-Repudiation of Receipt per
//!    BDEW AS4-Profil §2.2.4.  Without credentials the receipt is **unsigned**
//!    (dev/test only) and a warning is logged.
//!
//! # Security notes
//!
//! - Signing is **mandatory** for BDEW production deployments
//!   (`As4PushPolicy::regulated()`), including receipt signing — configure
//!   `with_receipt_credentials` with the operator's signing key pair.
//! - The dedup bridge is backed by SlateDB (single-node durable storage).
//!   Multi-replica deployments should replace it with a distributed backend.
//! - Receipt `message_id` values are UUID v4, scoped with a `makod@` prefix
//!   to distinguish them from sender-generated IDs.
//!
//! # Layering
//!
//! `mako-as4` remains a pure BDEW protocol/profile crate (no engine dep).
//! This module is the glue layer in `makod` that knows about both the engine
//! (`InboxStore`) and the transport (`asx_rs`).

use std::sync::Arc;

use asx_rs::as4::{
    As4ReceiptCredentials, As4ReceiveOutcome, As4ReceivePush, As4ReceivePushRequest,
    As4SignerPinMap, generate_receipt_for_output, generate_signed_receipt_for_output, receive_push,
};
use asx_rs::core::{AsxError, ErrorCode, ErrorContext, SessionContext};
use asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile;
use asx_rs::observability::EventBus;
use asx_rs::storage::{BoxFuture, DedupStorage, DedupVerdict};
use asx_rs::transport::ingress::As4HttpIngress;
use asx_rs::transport::server::{As4AxumHandler, HandlerOutcome, as4_router};
use axum::Router;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_as4::server::RouterConfig;
use mako_engine::inbox::{InboxClaim, InboxStore};
use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::SlateDbInboxStore;
use uuid::Uuid;

use crate::edifact_api::{EdifactApiState, MessageStatus};

// ── Dedup bridge ──────────────────────────────────────────────────────────────

/// Adapts [`SlateDbInboxStore`] to the async [`asx_rs::storage::DedupStorage`]
/// interface required by the AS4 receive pipeline.
///
/// `DedupStorage::first_seen` returns a `BoxFuture`, so `InboxStore::accept` is
/// called directly, without `block_in_place` / `block_on` boilerplate.
///
/// # Cluster safety
///
/// `cluster_safe()` returns `false` because SlateDB is a single-process store.
/// For multi-replica deployments, replace this with a distributed dedup backend
/// (Redis, PostgreSQL) that implements `DedupStorage` with `cluster_safe = true`.
pub struct SlateDbDedupBridge {
    store: Arc<SlateDbInboxStore>,
    /// `true` when the underlying SlateDB is backed by a persistent store
    /// (`--data-dir` or cloud object store). `false` when running in volatile
    /// (in-memory) mode — inbox dedup state is lost on restart and every AS4
    /// retry within the 72-hour window will re-process as a new message.
    ///
    /// Set from the `makod` configuration at startup so that the `asx-rs`
    /// receive pipeline can surface appropriate diagnostics via `is_durable()`.
    durable: bool,
}

impl SlateDbDedupBridge {
    pub fn new(store: Arc<SlateDbInboxStore>, durable: bool) -> Self {
        Self { store, durable }
    }
}

/// Hand-written, because the SlateDB handle behind the store is not printable
/// and a dedup backend's interesting property is its durability rather than its
/// file descriptors. `asx-rs` requires `Debug` on the trait so that the request
/// and policy types holding a backend can be printed.
impl std::fmt::Debug for SlateDbDedupBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlateDbDedupBridge")
            .field("durable", &self.durable)
            .field("cluster_safe", &false)
            .finish()
    }
}

impl DedupStorage for SlateDbDedupBridge {
    fn is_durable(&self) -> bool {
        // Report true only when the underlying SlateDB is backed by a
        // persistent store. In volatile (in-memory) mode, inbox dedup state is
        // lost on restart; `asx-rs` uses this flag to surface a diagnostic so
        // operators know that message idempotency is not guaranteed across
        // restarts. The `makod`-level VOLATILE MODE warning is still emitted at
        // startup regardless of this flag.
        self.durable
    }

    fn cluster_safe(&self) -> bool {
        false
    }

    fn claim<'a>(
        &'a self,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, asx_rs::core::Result<DedupVerdict>> {
        let store = Arc::clone(&self.store);
        let key = idempotency_key.to_owned();
        Box::pin(async move {
            store
                .claim(&key)
                .await
                .map(|verdict| match verdict {
                    InboxClaim::Claimed => DedupVerdict::Claimed,
                    InboxClaim::InFlight => DedupVerdict::InFlight,
                    InboxClaim::Duplicate => DedupVerdict::Duplicate,
                })
                .map_err(|e| dedup_err("claim", &e))
        })
    }

    fn accept<'a>(&'a self, idempotency_key: &'a str) -> BoxFuture<'a, asx_rs::core::Result<()>> {
        let store = Arc::clone(&self.store);
        let key = idempotency_key.to_owned();
        Box::pin(async move {
            store
                .accept(&key)
                .await
                .map_err(|e| dedup_err("accept", &e))
        })
    }

    fn abandon<'a>(&'a self, idempotency_key: &'a str) -> BoxFuture<'a, asx_rs::core::Result<()>> {
        let store = Arc::clone(&self.store);
        let key = idempotency_key.to_owned();
        Box::pin(async move {
            store
                .abandon(&key)
                .await
                .map_err(|e| dedup_err("abandon", &e))
        })
    }
}

/// Wrap an inbox failure as an `asx-rs` storage error.
///
/// Fail-closed by construction: `asx-rs` treats an error from any phase as a
/// refusal rather than as "first seen", so a storage outage cannot be mistaken
/// for a new message.
fn dedup_err(phase: &str, e: &mako_engine::error::EngineError) -> AsxError {
    AsxError::new(
        ErrorCode::StorageBackendFailure,
        format!("inbox dedup store error during {phase}: {e}"),
        ErrorContext::new("as4_dedup"),
    )
}

// ── AS4 receive handler ───────────────────────────────────────────────────────

/// AS4 inbound message handler for BDEW MaKo.
///
/// Shared across concurrent requests via `Arc<BdewAs4IngestHandler>`.
/// All fields are `Send + Sync`.
pub struct BdewAs4IngestHandler {
    /// Shared EDIFACT dispatch state (Platform + PidRouter).
    ingest: Arc<EdifactApiState>,
    /// Per-session WS-Security context (signing key + partner trust anchors).
    ///
    /// The **base** session. It carries no pinned signer fingerprint, which is
    /// why it is never handed to the receive pipeline directly: `asx-rs`
    /// compares the WS-Security signer against `cert_handle.fingerprint_sha256`
    /// and refuses a signed message outright when none is set. Every inbound
    /// message is verified against it with the claimed sender's permitted
    /// signing certificates supplied separately — see
    /// [`BdewAs4IngestHandler::with_sender_pins`].
    session: Arc<SessionContext>,
    /// Which signing certificates each counterparty may use, by MP-ID.
    ///
    /// The trust anchor answers „is the signer a BDEW/DVGW market
    /// participant?" — roughly 1500 parties hold a certificate chaining to it,
    /// so on its own it authenticates the PKI rather than the counterparty.
    /// These pins answer „is the signer *the party this message claims to come
    /// from*?", which is the question a Lieferantenwechsel depends on.
    ///
    /// `asx-rs` resolves them against the parsed `eb:From` and compares the
    /// verified signer itself, so a message claiming to be someone else is
    /// refused by the party whose pin it selected. A party with no pin has no
    /// permitted signer and is refused.
    ///
    /// `None` falls back to the session's single fingerprint, which serves one
    /// counterparty only.
    signer_pins: Option<Arc<dyn asx_rs::as4::As4SignerPins>>,
    /// Telemetry event bus.
    event_bus: Arc<EventBus>,
    /// Deduplication backend.
    dedup: Arc<dyn DedupStorage>,
    /// Operator's own AS4 inbound decryption private key (EC, BrainpoolP256r1).
    ///
    /// Per BDEW AS4-Profil v1.2 §2.2.6.2.2 inbound messages are encrypted with
    /// the operator's EC public key. asx-rs v0.7 decrypts them via ECDH-ES +
    /// ConcatKDF when an EC private key is supplied.
    decryption_key_pem: Option<std::sync::Arc<[u8]>>,
    /// Operator signing credentials for outbound `eb:Receipt` signals.
    ///
    /// Per BDEW AS4-Profil §2.2.4 receipts must be signed (Non-Repudiation of
    /// Receipt) and echo the inbound message's signed digests as
    /// NonRepudiationInformation.  When `None`, receipts are emitted
    /// **unsigned** — strict counterparties will reject them; dev/test only.
    receipt_credentials: Option<As4ReceiptCredentials>,
    /// CONTRL emitter — Empfangsbestätigung and Syntaxfehlermeldung.
    ///
    /// Per CONTRL AHB 1.0 §2.3.1 and APERAK AHB 1.0 §2.3 (Gas rules), a CONTRL
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
    pub fn new(
        ingest: Arc<EdifactApiState>,
        session: Arc<SessionContext>,
        event_bus: Arc<EventBus>,
        dedup: Arc<dyn DedupStorage>,
    ) -> Self {
        Self {
            ingest,
            session,
            signer_pins: None,
            event_bus,
            dedup,
            decryption_key_pem: None,
            receipt_credentials: None,
            contrl_ack: None,
        }
    }

    /// Pin each counterparty's signing certificate, by MP-ID.
    ///
    /// `certs` maps an MP-ID to that partner's signing certificate in PEM. A
    /// message is accepted only when the signer of its WS-Security signature is
    /// one of the certificates pinned for the party it claims to come from.
    ///
    /// Several certificates may be pinned for one party, which is what a
    /// certificate rollover needs: pin both for the overlap, then drop the old
    /// one.
    ///
    /// Without a pin for the party a message claims, the message is refused —
    /// there is no "unknown sender" mode, because the two failure directions
    /// are not symmetric. Accepting an unpinned sender lets any holder of any
    /// certificate the BDEW/DVGW PKI issued present themselves as any market
    /// participant, and a forged 55004 Abmeldung or ORDERS 17115 Sperrung
    /// cannot be withdrawn once acted on.
    ///
    /// # Errors
    ///
    /// Returns an error naming the MP-ID whose certificate could not be read.
    pub fn with_sender_pins<I, K, V>(mut self, certs: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: AsRef<str>,
    {
        let mut map = As4SignerPinMap::new();
        let mut pinned = 0usize;
        for (mp_id, pem) in certs {
            let mp_id = mp_id.into();
            map = map
                .pin_cert_pem(&mp_id, pem.as_ref().as_bytes())
                .map_err(|e| {
                    anyhow::anyhow!("AS4 partner signing certificate for MP-ID {mp_id}: {e}")
                })?;
            pinned += 1;
        }
        if pinned > 0 {
            self.signer_pins = Some(Arc::new(map));
        }
        Ok(self)
    }

    /// Set the operator's own AS4 inbound decryption private key.
    ///
    /// When set, `As4PushPolicy.inbound_decryption_key_pem` is populated so
    /// inbound encrypted AS4 messages can be decrypted. The key must be EC
    /// (BrainpoolP256r1) corresponding to the encryption certificate published
    /// to BDEW trading partners.
    #[must_use]
    pub fn with_decryption_key_pem(mut self, key_pem: Option<Vec<u8>>) -> Self {
        self.decryption_key_pem = key_pem.map(|k| std::sync::Arc::from(k.as_slice()));
        self
    }

    /// Set the operator signing credentials for outbound `eb:Receipt` signals.
    ///
    /// When set, synchronous receipts are signed (WS-Security XML Signature,
    /// `X509PKIPathv1` key-info per BDEW AS4-Profil §2.2.6.2.1) and carry the
    /// inbound message's `ds:Reference` digests as NonRepudiationInformation
    /// — satisfying Non-Repudiation of Receipt per BDEW AS4-Profil §2.2.4.
    ///
    /// **Required for BDEW production deployments.** Without it, receipts are
    /// emitted unsigned and strict counterparties will reject them.
    #[must_use]
    pub fn with_receipt_credentials(
        mut self,
        signing_key_pem: Vec<u8>,
        signing_cert_pem: Vec<u8>,
    ) -> Self {
        self.receipt_credentials = Some(As4ReceiptCredentials {
            signing_key_pem,
            signing_cert_pem,
            key_info_profile: WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        });
        self
    }

    /// Wire a `ContrlAckService` to emit Gas CONTRL Empfangsbestätigungen
    /// on every inbound Gas interchange (CONTRL AHB 1.0 §2.3.1).
    #[must_use]
    pub fn with_contrl_ack(mut self, svc: Arc<crate::contrl_ack::ContrlAckService>) -> Self {
        self.contrl_ack = Some(svc);
        self
    }
}

impl As4AxumHandler for BdewAs4IngestHandler {
    async fn handle(&self, ingress: As4HttpIngress) -> HandlerOutcome {
        let request = As4ReceivePushRequest {
            http_content_type: ingress.content_type.clone(),
            payload: Arc::clone(&ingress.body),
            receipt_payload: None,
            policy: mako_as4::bdew_push_policy(
                self.decryption_key_pem
                    .as_ref()
                    .map(|k| k.as_ref().to_vec()),
            ),
            authenticated_sender_scope: None,
        };

        match receive_push(
            &self.session,
            &self.event_bus,
            As4ReceivePush {
                request,
                dedup: Arc::clone(&self.dedup),
                // Fragment reassembly is not enabled, so no ordering context.
                ordering: None,
                // The pins are what bind a claimed `eb:From` to a verified
                // signer. `None` would fall back to the session's single
                // fingerprint, which cannot serve more than one counterparty.
                signer_pins: self.signer_pins.clone(),
            },
        )
        .await
        .map(asx_rs::as4::As4Ordered::into_inner)
        {
            Err(e) => {
                tracing::warn!(
                    error      = %e,
                    error_code = ?e.code,
                    "AS4 inbound: receive pipeline failed",
                );
                HandlerOutcome::bad_request(format!("AS4 receive failed: {e}"))
            }

            Ok(As4ReceiveOutcome::Duplicate { message_id }) => {
                // Idempotent replay: the dedup store has already seen this
                // message_id.  Per BDEW AS4 §4.3 retransmissions must receive
                // a valid acknowledgement — return 200 without re-dispatching.
                //
                // **A `400` from below is not a second chance.** The receive
                // pipeline consumes the dedup key before the outcome reaches
                // this handler, so it is spent by the time anything dispatches.
                // The BDEW-mandated `ReceptionAwareness` retry carries the same
                // `eb:MessageId`, so a retransmission lands here — 200, no
                // dispatch — and the counterparty is told the message was
                // delivered. Anything that fails after the key is spent must
                // therefore leave a dead letter: it will not come round again.
                tracing::debug!(
                    as4_message_id = %message_id,
                    "AS4 inbound: duplicate detected — returning idempotent 200",
                );
                HandlerOutcome::ok()
            }

            Ok(As4ReceiveOutcome::FirstSeen { output, claim }) => {
                let outcome = self.handle_first_seen(output, &ingress).await;

                // Settle the claim as handled, on every path.
                //
                // By the time `handle_first_seen` returns, the message is
                // durably recorded whichever way it went: dispatched, or
                // dead-lettered before the refusal. `check-as4-controls`
                // enforces that second half, because it is what makes this
                // single unconditional `accept` correct — a refusal path that
                // returned without a dead letter would be accepted here and the
                // message would be gone, its retransmission answered as a
                // duplicate.
                if let Err(e) = claim.accept().await {
                    // The work is done and recorded; only the dedup settlement
                    // failed. Saying so is all that is left — the claim's lease
                    // expires and a retransmission is reprocessed, which is the
                    // safe direction.
                    tracing::error!(
                        error = %e,
                        "AS4 inbound: could not settle the dedup claim; a retransmission \
                         will be reprocessed once the claim lease expires",
                    );
                }
                outcome
            }

            Ok(As4ReceiveOutcome::InFlight { message_id }) => {
                // A concurrent delivery of this same message holds an unsettled
                // claim. Not a duplicate — nothing has been handled yet, so an
                // acknowledgement would tell the counterparty a message was
                // processed that may still fail — and not first-seen either.
                // Refusing without a receipt leaves the retransmission as the
                // recovery path, which is what it is for.
                tracing::warn!(
                    as4_message_id = %message_id,
                    "AS4 inbound: a concurrent delivery holds this message; refusing without \
                     a receipt so the retransmission is the recovery path",
                );
                HandlerOutcome::bad_request(
                    "a concurrent delivery of this eb:MessageId is still in flight",
                )
            }

            // `As4ReceiveOutcome` is `#[non_exhaustive]` — keep a catch-all for
            // future variants so new protocol outcomes don't silently fall through.
            Ok(_) => {
                tracing::warn!("AS4 inbound: unhandled receive outcome variant");
                HandlerOutcome::ok()
            }
        }
    }
}

// ── Dispatch failure dead-letter ──────────────────────────────────────────────

/// Record a Phase-2 dispatch failure as a dead letter (§ 147 AO / GoBD).
///
/// A failed dispatch is the message's last chance: the AS4 dedup entry is
/// already durable, so the sender's 72-hour retries come back as duplicates,
/// and the synchronous receipt tells it the message arrived. Without this entry
/// the message would simply be gone — the same reason the parse-error and
/// unknown-PID paths dead-letter.
fn dead_letter_dispatch_failure(
    sink: &dyn mako_engine::dead_letter::DeadLetterSink,
    as4_message_id: &str,
    message_type: Option<&str>,
    pid: mako_engine::ids::Pid,
    workflow: &str,
    error: &mako_engine::error::EngineError,
) {
    use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
    sink.reject(&DeadLetterReason::ProcessingError {
        message: format!("dispatch_failed: workflow {workflow}: {error}"),
        context: AuditContext::now()
            .with_message_type(message_type.unwrap_or(""))
            .with_pid(pid)
            .with_message_ref(as4_message_id),
    });
}

// ── Router builder ────────────────────────────────────────────────────────────

/// Build the Axum sub-router that mounts the BDEW AS4 inbound endpoint.
///
/// The router accepts `POST /as4/inbox` with `Content-Type: application/soap+xml`.
/// Merge it into the top-level application router:
///
/// ```ignore
/// app = app.merge(as4_ingest::router(handler, config));
/// ```
pub fn router(handler: Arc<BdewAs4IngestHandler>, config: RouterConfig) -> Router {
    as4_router(handler, "/as4/inbox", config)
}

// ── AS4 inbound rate-limit middleware ─────────────────────────────────────────

/// Per-peer-IP GCRA rate limiter for the AS4 inbound endpoint.
///
/// **Keyed by client IP**: each peer gets its own token bucket (sustained
/// **100 requests/second**, burst **50**), so one noisy or malicious
/// counterparty cannot exhaust the budget for everyone else — the failure
/// mode of a single global bucket. Returns `HTTP 429 Too Many Requests` when
/// a peer's bucket is empty, protecting the event store from capacity
/// exhaustion (OWASP A05).
///
/// The key is the socket peer address (`ConnectInfo`); deployments behind a
/// load balancer terminate client connections there, so the LB must enforce
/// its own per-client limits (`X-Forwarded-For` is spoofable and deliberately
/// not trusted here).
/// How many checks pass between sweeps of a keyed limiter's state.
///
/// `governor`'s keyed store grows one entry per distinct key and is pruned only
/// by an explicit `retain_recent()`; nothing calls it on a timer. Sweeping on a
/// request count rather than a clock keeps the cost proportional to traffic and
/// needs no background task to own — and so no shutdown path to get wrong.
///
/// Stated here rather than taken from `mako_service::rate_limit`, which lives
/// behind the `rate-limit` feature: makod drives `governor` directly and has no
/// other reason to pull in that middleware.
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
/// **It is established immediately after.** The same claimed `eb:From` selects
/// the session the message is verified against, and that session pins the
/// SHA-256 of the claimed party's signing certificate — so a sender claiming to
/// be someone else selects that party's pin and then fails to produce a
/// matching signature. Chaining to the BDEW/DVGW trust anchor is not that
/// binding on its own: every market participant holds such a certificate, so
/// the anchor proves membership and the pin proves membership *of which party*.
/// A claimed MP-ID with no registered signing certificate is refused outright.
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
/// **Deliberately not [`AS4_RATE_LIMITER`].** The REST API and the
/// API-Webdienste port used to share the AS4 bucket, on the reasoning that a
/// rate limit is a property of the peer rather than of the endpoint. That holds
/// for a direct connection and breaks behind a proxy, which is the documented
/// deployment: every client of every port arrives from the ingress address, so
/// all three collapsed into one 100 req/s bucket. An ERP batch on `:8080` could
/// then `429` a trading partner's AS4 delivery — and that partner's retry
/// schedule is the thing standing between us and a missed Frist. The AS4 budget
/// is now reserved for counterparties.
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

    use super::dead_letter_dispatch_failure;

    #[derive(Default)]
    struct CapturingSink(Mutex<Vec<(String, String)>>);

    impl DeadLetterSink for CapturingSink {
        fn reject(&self, reason: &DeadLetterReason) {
            let detail = match reason {
                DeadLetterReason::ProcessingError { message, .. } => message.clone(),
                other => format!("{other:?}"),
            };
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((reason.label().to_owned(), detail));
        }
    }

    /// A dispatch failure must leave a dead letter naming it. Logged as
    /// "non-fatal" while the receipt still goes out and the dedup entry stays
    /// durable, the message is lost with no § 147 AO trace.
    #[test]
    fn a_failed_dispatch_is_dead_lettered() {
        let sink = CapturingSink::default();
        dead_letter_dispatch_failure(
            &sink,
            "as4-msg-1",
            Some("UTILMD"),
            mako_engine::ids::Pid::new(55001),
            "gpke-supplier-change",
            &mako_engine::error::EngineError::Registry {
                message: "store unavailable".to_owned(),
                transient: false,
            },
        );
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
    /// Handle a verified, first-seen inbound message.
    ///
    /// Extracted from the receive match so the dedup claim is settled in exactly
    /// one place. Inline, every one of this body's return points would have to
    /// remember to settle, and the one that forgot would strand the key until
    /// its lease expired.
    ///
    /// **Every path here records the message durably before returning** —
    /// dispatched, or dead-lettered ahead of the refusal — which is what lets
    /// the caller settle unconditionally. `check-as4-controls` keeps it true.
    async fn handle_first_seen(
        &self,
        output: Box<asx_rs::as4::As4ReceivePushOutput>,
        ingress: &As4HttpIngress,
    ) -> HandlerOutcome {
        let msg_id = output.user_message.message_id.clone();
        let action = &output.user_message.action;
        let from = output
            .user_message
            .from_party_ids
            .first()
            .map(String::as_str)
            .unwrap_or("<unknown>");
        let edifact = output.payload.clone().into_inner();

        tracing::info!(
            as4_message_id = %msg_id,
            action         = %action,
            from_party     = %from,
            payload_bytes  = edifact.len(),
            "AS4 inbound: message received",
        );

        // ── Synchronous receipt builder ───────────────────────────────
        // BDEW AS4-Profil §2.2.4: the receipt must be signed and echo
        // the inbound message's ds:Reference digests (NRR).  Unsigned
        // receipts are a dev/test fallback only — strict counterparties
        // reject them.  Shared by the EDIFACT and Redispatch-XML legs.
        let send_receipt = || {
            let receipt_id = format!("makod@{}", Uuid::new_v4());
            let receipt = match &self.receipt_credentials {
                Some(credentials) => generate_signed_receipt_for_output(
                    &self.session,
                    &receipt_id,
                    &output,
                    &ingress.body,
                    &ingress.content_type,
                    credentials,
                ),
                None => {
                    tracing::warn!(
                        as4_message_id = %msg_id,
                        "AS4 inbound: no receipt-signing credentials configured — \
                         emitting UNSIGNED receipt without NRI. This violates BDEW \
                         AS4-Profil §2.2.4; configure with_receipt_credentials for \
                         production.",
                    );
                    generate_receipt_for_output(&self.session, &receipt_id, &output)
                }
            };
            match receipt {
                Ok(receipt_xml) => {
                    tracing::debug!(
                        as4_message_id = %msg_id,
                        receipt_id     = %receipt_id,
                        signed         = self.receipt_credentials.is_some(),
                        "AS4 inbound: sending synchronous receipt",
                    );
                    HandlerOutcome::ok_with_body(receipt_xml, "application/soap+xml")
                }
                Err(e) => {
                    // Receipt generation failure is non-fatal for the
                    // business payload — message was already dispatched.
                    // Return 200 without a receipt body; the sender will
                    // retry and hit the dedup path.
                    tracing::error!(
                        as4_message_id = %msg_id,
                        error          = %e,
                        "AS4 inbound: receipt generation failed — returning 200 without body",
                    );
                    HandlerOutcome::ok()
                }
            }
        };

        // ── ebMS3 Test Service (Core §5.2.2) ──────────────────────────
        // A connectivity ping is acknowledged and **not delivered**: its
        // payload is empty or a loopback of what the sender sent, so
        // every branch below would read it as a malformed business
        // document — a dead letter, or a 400 that tells the counterparty
        // its connectivity check failed. `is_test_service_ping` is
        // derived from the verified `eb:Service` and `eb:Action`, so a
        // sender cannot label a real document as a ping to skip the
        // pipeline: the payload never reaches it either way.
        if output.is_test_service_ping() {
            tracing::info!(
                as4_message_id = %msg_id,
                from_party     = %from,
                "AS4 inbound: ebMS3 Test Service ping — acknowledged, not delivered",
            );
            return send_receipt();
        }

        // ── Redispatch 2.0 XML leg ────────────────────────────────────
        // The BDEW AS4 channel carries two payload formats: EDIFACT
        // interchanges and the nine Redispatch 2.0 XML document types
        // (BK6-20-059/-061).  XML payloads never enter the EDIFACT
        // pipeline — no UNB envelope, no test indicator, no CONTRL
        // obligation.
        if crate::redispatch_xml_ingest::looks_like_xml(&edifact) {
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
                    });
                tracing::error!(
                    as4_message_id = %msg_id,
                    "AS4 ingest: XML payload received but no Phase 2 dispatcher \
                     is wired — dead-lettered",
                );
                return HandlerOutcome::bad_request(
                    "XML payload received but workflow dispatch is not configured",
                );
            };
            return match crate::redispatch_xml_ingest::dispatch_redispatch_xml(dispatcher, &edifact)
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
                        });
                    tracing::warn!(
                        as4_message_id = %msg_id,
                        workflow       = %workflow_name,
                        reason         = %reason,
                        "AS4 ingest: Redispatch XML payload rejected — dead-lettered",
                    );
                    HandlerOutcome::bad_request(format!("Redispatch XML rejected: {reason}"))
                }
                Ok(outcome) => {
                    tracing::info!(
                        as4_message_id = %msg_id,
                        outcome        = ?outcome,
                        "AS4 ingest: Redispatch XML document dispatched",
                    );
                    send_receipt()
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
                        });
                    tracing::error!(
                        as4_message_id = %msg_id,
                        error          = %e,
                        "AS4 ingest: Redispatch XML dispatch failed — dead-lettered",
                    );
                    HandlerOutcome::bad_request(format!("Redispatch XML dispatch failed: {e}"))
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
        let unb = self
            .ingest
            .platform
            .parse_interchange_header(&edifact[..])
            .ok();

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
                });
            tracing::warn!(
                as4_message_id = %msg_id,
                sender = %header.sender_id,
                receiver = %header.receiver_id,
                control_ref = %header.control_ref,
                "AS4 ingest: interchange is addressed to an MP-ID this deployment \
                 does not hold — refused",
            );
            return HandlerOutcome::bad_request(
                "UNB DE0010 names an MP-ID this deployment does not hold",
            );
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
                .reject(&DeadLetterReason::TestMessage { context: ctx });
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
                        &edifact,
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
                    });
            }
            return HandlerOutcome::bad_request(
                "test interchange rejected: DE0035=1 on production endpoint",
            );
        }
        // ── DVGW gas transport ────────────────────────────────────────
        // Tried first, because a DVGW message rides `ORDERS`/`ORDRSP` and
        // the BDEW parser would accept it as one — yielding a
        // Prüfidentifikator from the wrong catalogue. `try_ingest` sniffs
        // `BGM` DE 1001 and returns `None` for a BDEW interchange, which
        // costs that path only the sniff.
        {
            if let Some(report) =
                crate::dvgw_ingest::try_ingest(self.ingest.as_ref(), &edifact).await
            {
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
                            });
                    }
                }

                // The AS4 receipt is owed regardless of which family the
                // payload belongs to, so the DVGW path returns through the
                // same closure the BDEW path ends with rather than
                // short-circuiting it.
                return send_receipt();
            }
        }

        // ── EDIFACT dispatch ──────────────────────────────────────────
        // Collect parsed messages so they can be passed to ContrlAckService
        // after the dispatch loop (CONTRL AHB 1.0 §2.3.1 Gas obligation).
        // The recipient MP-ID (UNB DE0010) drives Sparte detection and the
        // CONTRL sender MP-ID.
        let (interchange_ref, recipient_mp_id): (String, String) =
            if let Ok(pi) = self.ingest.platform.parse_interchange_full(&edifact[..]) {
                (
                    pi.header.control_ref.to_string(),
                    pi.header.receiver_id.to_string(),
                )
            } else {
                (msg_id.clone(), String::new())
            };
        let mut accepted = 0usize;
        let mut rejected = 0usize;
        let mut parsed_msgs: Vec<edi_energy::AnyMessage> = Vec::new();
        // `UCI` DE 0085 of the Syntaxfehlermeldung, if one is owed. The
        // segment carries one code, so it is the first fault that names
        // the file (§5.3.3 reports at the lowest level that can express
        // a fault, and an interchange-level UCI has no lower level here).
        let mut first_syntax_error = crate::contrl_ack::SyntaxFehler::UngueltigerWert;
        let mut saw_syntax_error = false;
        for result in self
            .ingest
            .platform
            .parse_interchange(std::io::Cursor::new(&edifact[..]))
        {
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
                            });
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
                        self.ingest.dl_sink.reject(&reason);
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
                        match dispatcher.dispatch(&msg, wf_name, pid_val.as_u32()).await {
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
                                    self.ingest.dl_sink.reject(
                                        &DeadLetterReason::NotDispatchable {
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
                                        },
                                    );
                                }
                                tracing::debug!(
                                    as4_message_id = %msg_id,
                                    workflow       = %wf_name,
                                    outcome        = ?outcome,
                                    "AS4 ingest: Phase 2 command dispatched",
                                );
                            }
                            Err(e) => {
                                dead_letter_dispatch_failure(
                                    self.ingest.dl_sink.as_ref(),
                                    &msg_id,
                                    message_type.as_deref(),
                                    pid_val,
                                    wf_name,
                                    &e,
                                );
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
                && let Ok(pi) = self.ingest.platform.parse_interchange_full(&edifact[..])
                && let Err(e) = contrl_svc
                    .emit_syntax_error(
                        &edifact,
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
                    });
            }
            return HandlerOutcome::bad_request("AS4 payload contained no valid EDIFACT messages");
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
                    });
            }
        }

        // ── Synchronous receipt (BDEW AS4-Profil §2.2.4) ──────────────
        send_receipt()
    }
}
