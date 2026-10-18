//! The AS4 boundary on `asx-rs` 0.16: makod's clients against a BDEW partner
//! client over an in-process loopback, with test SM-PKI material.
//!
//! The partner is a real `asx::as4::As4` client of its own; nothing here
//! weakens a check. `asx::testkit::pki::TestCa::sm_pki()` plays the SM-PKI
//! root and issues each MP-ID separate signing, encryption and TLS key pairs
//! with the MP-ID as subject OU; `asx::testkit::loopback::Loopback` carries
//! the requests and serves each test CA's CRL.
//!
//! | Test | Contract |
//! |---|---|
//! | `slatedb_dedup_conforms` | `SlateDbDedup` is an `asx::DedupStore`: atomic claim, token-fenced settlement |
//! | `an_outbound_message_is_delivered_with_its_receipt_evidence` | send: Service MP, `{outbox id}@{domain}`, §2.12 file name, receipt evidence kept |
//! | `a_retry_resends_the_first_attempts_message_id_and_timestamp` | the retry's `eb:MessageId`, `eb:Timestamp` and digests are the first attempt's |
//! | `a_partner_refusal_is_retried_and_a_receipt_failure_is_not_acknowledged` | delivery errors stay in the outbox |
//! | `an_interchange_from_a_configured_partner_is_routed` | inbound: verified, dispatched, receipted |
//! | `a_refusal_is_dead_lettered_before_the_claim_is_released_for_good` | dead letter before the claim settles, `EBMS:0004` not retried |
//! | `an_unrecorded_refusal_is_retryable` | dead-letter write fails → `EBMS:0202`, the retry is reprocessed |
//! | `an_accepted_interchange_settles_after_its_dead_letters` | dead letters before `accept` |
//! | `an_unconfigured_sender_is_refused_under_nur_konfigurierte` | `[policy.as4] unbekannte_absender` default |
//! | `an_unconfigured_sender_is_served_under_bdew_ou_zulassen` | §2.3.2 option |
//! | `a_tampered_request_is_refused_ebms_0101` | signature integrity |

use std::sync::{Arc, Mutex};

use asx::testkit::loopback::Loopback;
use asx::testkit::pki::{KeyAlg, TestCa, TestParty};
use asx::{BoxFuture, Claim, ClaimToken, DedupStore};
use mako_as4::{LocalIdentity, MessageIdDomain, PartnerDirectory, PartnerEntry};
use mako_engine::dead_letter::{DeadLetterReason, DeadLetterSink, RejectFuture};
use mako_engine::error::EngineError;
use mako_engine::store_slatedb::SlateDbStore;
use makod::as4_clients::{As4Clients, As4Setup};
use makod::as4_evidence::As4Evidence;
use makod::as4_ingest::{self, BdewAs4IngestHandler, SlateDbDedup};
use makod::config::UnbekannteAbsender;

/// makod's own MP-ID (NB).
const OWN: &str = "9900001000001";
/// The configured partner (LF).
const PARTNER: &str = "9900001000002";
/// A market participant with an SM-PKI certificate and no `[[as4.partner]]`.
const STRANGER: &str = "9900001000003";

const OWN_URL: &str = "https://makod.test/as4/inbox/9900001000001";
const PARTNER_URL: &str = "https://partner.test/as4/inbox/9900001000002";

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ── Fixture ───────────────────────────────────────────────────────────────────

/// One SM-PKI, makod's party, the partner and a stranger.
struct Pki {
    ca: TestCa,
    crl_signer: asx::Certificate,
    own: TestParty,
    partner: TestParty,
    stranger: TestParty,
}

impl Pki {
    fn new() -> Self {
        let ca = TestCa::sm_pki();
        let crl_signer = ca
            .crl_signer("CCRL-S(Root)", KeyAlg::EcBrainpoolP256r1)
            .certificate();
        Self {
            own: ca.bdew_party(OWN),
            partner: ca.bdew_party(PARTNER),
            stranger: ca.bdew_party(STRANGER),
            crl_signer,
            ca,
        }
    }

    /// makod's clients, built the way the preflight builds them.
    fn makod(&self, loopback: &Loopback, policy: UnbekannteAbsender) -> As4Clients {
        let mut partners = PartnerDirectory::default();
        partners
            .insert(PartnerEntry {
                mp_id: PARTNER.to_owned(),
                agency: "293".to_owned(),
                endpoint: Some(PARTNER_URL.to_owned()),
                encryption_cert: Some(self.partner.encryption.certificate()),
                signing_certs: vec![self.partner.signing.certificate()],
                quirks: Vec::new(),
            })
            .unwrap();
        let setup = As4Setup {
            domain: MessageIdDomain::parse("as4.makod.test").unwrap(),
            trust_anchor: self.ca.anchor(),
            crl_signer: self.crl_signer.clone(),
            identities: vec![LocalIdentity {
                mp_id: OWN.to_owned(),
                agency: "293".to_owned(),
                signer: self.own.signing.key_pair(),
                decrypter: self.own.encryption.key_pair(),
                tls: self.own.tls.key_pair(),
            }],
            partners,
            unbekannte_absender: policy,
        };
        As4Clients::build_with_http(setup, loopback.clone()).expect("makod's clients build")
    }

    /// A counterparty's own `asx-rs` client, with makod as its partner.
    fn counterparty(&self, party: &TestParty, loopback: &Loopback) -> asx::as4::As4 {
        asx::as4::As4::builder(asx::Profile::bdew())
            .local(party.local())
            .trust_anchor(self.ca.anchor())
            .crl_signer(self.crl_signer.clone())
            .partner(self.own.partner(OWN_URL))
            .http_client(loopback.clone())
            .build()
            .expect("the counterparty builds")
    }
}

/// The settlement log shared by the recording dedup store and dead-letter
/// sink, so a test reads the order of the two.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<&'static str>>>);

impl Log {
    fn push(&self, e: &'static str) {
        self.0.lock().unwrap().push(e);
    }
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// `SlateDbDedup` that logs every phase it is asked for.
#[derive(Clone, Debug)]
struct RecordingDedup {
    inner: SlateDbDedup,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl DedupStore for RecordingDedup {
    fn claim<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Claim, BoxError>> {
        self.log.lock().unwrap().push("claim");
        self.inner.claim(key)
    }
    fn accept<'a>(
        &'a self,
        key: &'a str,
        t: &'a ClaimToken,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        self.log.lock().unwrap().push("accept");
        self.inner.accept(key, t)
    }
    fn abandon<'a>(
        &'a self,
        key: &'a str,
        t: &'a ClaimToken,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        self.log.lock().unwrap().push("abandon");
        self.inner.abandon(key, t)
    }
}

/// A dead-letter sink that logs, and fails while `fail` is set.
struct RecordingSink {
    log: Log,
    fail: Arc<std::sync::atomic::AtomicBool>,
    reasons: Mutex<Vec<String>>,
}

impl DeadLetterSink for RecordingSink {
    fn reject<'a>(&'a self, reason: &'a DeadLetterReason) -> RejectFuture<'a> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            self.log.push("dead_letter_failed");
            return Box::pin(std::future::ready(Err(EngineError::dead_letter(
                "the dead-letter store is closed",
            ))));
        }
        self.log.push("dead_letter");
        self.reasons.lock().unwrap().push(reason.to_string());
        Box::pin(std::future::ready(Ok(())))
    }
}

fn registry() -> Arc<makod::party_registry::MpIdRegistry> {
    use makod::config::PartyConfig;
    Arc::new(
        makod::party_registry::MpIdRegistry::from_config(&[PartyConfig {
            mp_id: OWN.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .unwrap(),
    )
}

/// makod's inbox for `OWN` behind `loopback`, dispatching into `store` (or
/// routing nothing when `dispatch` is false), with the recording seams.
struct Inbox {
    log: Log,
    fail_dead_letters: Arc<std::sync::atomic::AtomicBool>,
    sink: Arc<RecordingSink>,
    store: SlateDbStore,
}

async fn mount_inbox(
    pki: &Pki,
    loopback: &Loopback,
    policy: UnbekannteAbsender,
    dispatch: bool,
) -> Inbox {
    let store = SlateDbStore::open_in_memory().await.unwrap();
    let registry = registry();
    let tenant = mako_engine::ids::TenantId::from_party_id(OWN);
    let log = Log::default();
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sink = Arc::new(RecordingSink {
        log: log.clone(),
        fail: Arc::clone(&fail),
        reasons: Mutex::new(Vec::new()),
    });
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    let dispatcher = if dispatch {
        // 55001 — Anmeldung Lieferbeginn (LF → NB), spawn-capable on the NB side.
        pid_router.register(55001, "gpke-supplier-change");
        Some(Arc::new(
            makod::ingest_dispatcher::EdifactIngestDispatcher::new(
                Arc::new(store.clone()),
                store.as_snapshot_store(),
                100,
                tenant,
            ),
        ))
    } else {
        None
    };
    let state = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
        mp_id_registry: Arc::clone(&registry),
        cedar: Arc::new(makod::cedar_authz::CedarAuthorizer::unauthenticated().unwrap()),
        max_body_bytes: 64 << 20,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: Arc::clone(&sink) as Arc<dyn DeadLetterSink>,
        dispatcher,
        contrl_ack: None,
    });
    let clients = pki.makod(loopback, policy);
    let handler = Arc::new(
        BdewAs4IngestHandler::new(state, OWN, clients.clone())
            .with_evidence(As4Evidence::new(store.clone())),
    );
    let dedup = RecordingDedup {
        inner: SlateDbDedup::new(store.as_inbox_store()),
        log: Arc::clone(&log.0),
    };
    loopback.mount(
        OWN_URL,
        as4_ingest::inbound(clients.client(OWN).unwrap(), dedup, handler),
    );
    Inbox {
        log,
        fail_dead_letters: fail,
        sink,
        store,
    }
}

/// A UTILMD 55001 Übertragungsdatei from `from` to `to`, rendered by makod's
/// own renderer with `from` as the sending party.
fn utilmd_55001(from: &str, to: &str) -> Vec<u8> {
    use makod::config::PartyConfig;
    let sender_registry = makod::party_registry::MpIdRegistry::from_config(&[PartyConfig {
        mp_id: from.to_owned(),
        roles: vec!["LF".to_owned()],
        primary: true,
        agency: None,
        as4: None,
    }])
    .unwrap();
    let msg = crate::support::outbox_message(
        "UTILMD",
        to,
        serde_json::json!({
            "pid": 55001,
            "sender": from,
            "receiver": to,
            "malo": "51238696781",
            "process_date": "2026-03-01",
            "bilanzkreis": "11XBK-EEG-----1",
            "transaktionsgrund": "E03",
            "kundengruppe": "Z15",
            "enfg_grundlage": "ZG0",
            "kunde_name": "Mustermann, Max",
            "kunde_anschrift": { "strasse": "Musterstr. 1", "plz": "10115", "ort": "Berlin", "land": "DE" },
        }),
    );
    makod::edifact_renderer::render_to_wire_bytes(&msg, &sender_registry)
        .expect("renders")
        .bytes
}

fn push(to: &asx::Partner, bytes: Vec<u8>, id: &str) -> asx::as4::Message {
    asx::as4::Message::to(to)
        .service(mako_as4::Service::Marktprozesse.uri())
        .message_id(format!("{id}@partner.test"))
        .payload(asx::Payload::new(
            bytes,
            mako_as4::constants::PAYLOAD_MIME_TYPE,
        ))
}

// ── Dedup ─────────────────────────────────────────────────────────────────────

/// `asx::testkit::dedup::conformance` over `SlateDbDedup`.
///
/// The SlateDB lease runs on the wall clock, so the store is given a one-second
/// lease and the suite runs unpaused (the suite's documented shape for a store
/// with its own clock). Every check opens a fresh, empty store.
#[tokio::test(flavor = "multi_thread")]
async fn slatedb_dedup_conforms() {
    struct Harness(Mutex<Vec<SlateDbDedup>>);
    impl std::fmt::Debug for Harness {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("SlateDbDedup harness")
        }
    }
    impl asx::testkit::dedup::DedupHarness for Harness {
        type Store = SlateDbDedup;
        fn open(&self) -> SlateDbDedup {
            self.0
                .lock()
                .unwrap()
                .pop()
                .expect("enough pre-opened stores")
        }
        fn lease(&self) -> std::time::Duration {
            std::time::Duration::from_secs(1)
        }
    }
    let mut stores = Vec::new();
    for _ in 0..(asx::testkit::dedup::CHECKS.len() * 2) {
        let store = SlateDbStore::open_in_memory().await.unwrap();
        stores.push(SlateDbDedup::new(
            store
                .as_inbox_store()
                .with_claim_lease(std::time::Duration::from_secs(1)),
        ));
    }
    let failures = asx::testkit::dedup::conformance(&Harness(Mutex::new(stores))).await;
    assert!(failures.is_empty(), "{failures:#?}");
}

// ── Outbound ──────────────────────────────────────────────────────────────────

/// One message the partner's handler saw: message id, service, payload,
/// payload name and the evidence as JSON.
type Seen = (String, String, Vec<u8>, Option<String>, serde_json::Value);

/// What the partner's handler saw.
#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<Seen>>>);

/// Mount the partner's inbox; its handler refuses (retryably) the first
/// `refuse_first` messages, then accepts.
fn mount_partner(pki: &Pki, loopback: &Loopback, refuse_first: usize) -> Received {
    let received = Received::default();
    let seen = received.clone();
    let partner = pki.counterparty(&pki.partner, loopback);
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    loopback.mount(
        PARTNER_URL,
        partner.inbound(
            asx::MemoryDedup::default(),
            move |msg: asx::Verified<asx::as4::UserMessage>| {
                let seen = seen.clone();
                let refused = Arc::clone(&refused);
                async move {
                    let payload = &msg.payloads()[0];
                    seen.0.lock().unwrap().push((
                        msg.message_id().to_owned(),
                        msg.service().to_owned(),
                        payload.bytes().to_vec(),
                        payload.name().map(str::to_owned),
                        serde_json::to_value(msg.evidence()).unwrap(),
                    ));
                    if refused.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < refuse_first {
                        return Err(asx::Reject::new("not yet"));
                    }
                    Ok(())
                }
            },
        ),
    );
    received
}

async fn sender(pki: &Pki, loopback: &Loopback) -> (makod::as4_sender::BdewAs4Sender, As4Evidence) {
    let store = SlateDbStore::open_in_memory().await.unwrap();
    let evidence = As4Evidence::new(store.clone());
    let malo_sender = makod::malo_ident_sender::MaloIdentSender::new(
        makod::malo_cache::SlateDbMaloCache::new(store.clone()),
        reqwest::Client::new(),
        std::collections::HashMap::new(),
        None,
        store.clone(),
        None,
    );
    let sender = makod::as4_sender::BdewAs4Sender::new(
        pki.makod(loopback, UnbekannteAbsender::NurKonfigurierte),
        evidence.clone(),
        malo_sender,
        registry(),
        None,
        Arc::new(edi_energy::Platform::with_all_profiles()),
    );
    (sender, evidence)
}

/// An APERAK 29002 from makod (NB) to the partner (LF), stamped as the
/// outbox worker stamps it.
fn aperak() -> mako_engine::outbox::OutboxMessage {
    let pending = mako_engine::outbox::PendingOutbox::aperak_anerkennung(OWN, PARTNER, "ORIGREF1");
    let mut msg =
        crate::support::outbox_message(&pending.message_type, &pending.recipient, pending.payload);
    msg.first_attempt_at = Some(
        time::OffsetDateTime::now_utc()
            .replace_millisecond(0)
            .unwrap(),
    );
    msg
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outbound_message_is_delivered_with_its_receipt_evidence() {
    use mako_engine::builder::As4Sender as _;
    let pki = Pki::new();
    let loopback = Loopback::new();
    let received = mount_partner(&pki, &loopback, 0);
    let (sender, evidence) = sender(&pki, &loopback).await;
    let msg = aperak();

    sender.send(&msg).await.expect("delivered and receipted");

    let got = received.0.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let (id, service, bytes, name, _) = &got[0];
    assert_eq!(id, &format!("{}@as4.makod.test", msg.message_id));
    // BDEW AS4-Profil 1.2 §2.3.1.2.1: the Marktprozesse service.
    assert_eq!(service, "https://www.bdew.de/as4/communication/services/MP");
    assert!(
        bytes.starts_with(b"UNA") || bytes.starts_with(b"UNB"),
        "the Übertragungsdatei"
    );
    assert!(
        name.is_none(),
        "asx-rs does not surface the part's file name inbound"
    );
    // The file name rides the part's Content-Disposition on the wire.
    let wire = loopback.last_request().expect("the request");
    let wire = String::from_utf8_lossy(wire.body());
    let expected = format!("filename=\"APERAK__{OWN}_{PARTNER}_");
    assert!(
        wire.contains(&expected),
        "Allgemeine Festlegungen V6.1d §2.12 file name on the payload part"
    );

    let record = evidence
        .outbound(&msg.message_id.to_string())
        .await
        .unwrap()
        .expect("the receipt evidence is kept");
    assert_eq!(record.message_id(), id);
    assert!(record.signal().is_some(), "the partner's signed receipt");
}

/// Every attempt of one outbox entry is the same AS4 message: id, timestamp
/// and the digests the signature covers. The partner's first answer is a
/// retryable refusal (`EBMS:0202`), so the second attempt reaches its handler.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_resends_the_first_attempts_message_id_and_timestamp() {
    use mako_engine::builder::As4Sender as _;
    let pki = Pki::new();
    let loopback = Loopback::new();
    let received = mount_partner(&pki, &loopback, 1);
    let (sender, _) = sender(&pki, &loopback).await;
    let msg = aperak();

    let first = sender
        .send(&msg)
        .await
        .expect_err("the partner refuses once");
    assert!(
        matches!(first, EngineError::Transport { .. }),
        "a partner refusal is retryable: {first:?}"
    );
    sender.send(&msg).await.expect("the retry is delivered");

    let got = received.0.lock().unwrap().clone();
    assert_eq!(got.len(), 2, "both attempts reached the partner");
    assert_eq!(got[0].0, got[1].0, "same eb:MessageId");
    let field = |i: usize, f: &str| got[i].4[f].clone();
    assert!(field(0, "message_timestamp").is_string());
    assert_eq!(
        field(0, "message_timestamp"),
        field(1, "message_timestamp"),
        "same eb:Timestamp"
    );
    assert_eq!(
        field(0, "references"),
        field(1, "references"),
        "same digests"
    );
}

/// A delivery whose receipt does not verify is not acknowledged: the outbox
/// keeps the entry and resends it.
#[tokio::test(flavor = "multi_thread")]
async fn a_partner_refusal_is_retried_and_a_receipt_failure_is_not_acknowledged() {
    use mako_engine::builder::As4Sender as _;
    let pki = Pki::new();
    let loopback = Loopback::new();
    mount_partner(&pki, &loopback, 0);
    let (sender, evidence) = sender(&pki, &loopback).await;
    // Corrupt every receipt the partner returns.
    loopback.intercept(|response| {
        let mut body = response.body().to_vec();
        if let Some(i) = body.windows(9).position(|w| w == b"Signature") {
            body[i + 20] ^= 0x01;
        }
        *response.body_mut() = body.into();
    });
    let msg = aperak();
    let err = sender
        .send(&msg)
        .await
        .expect_err("an unverifiable receipt is no delivery");
    assert!(matches!(err, EngineError::Transport { .. }), "{err:?}");
    assert!(
        evidence
            .outbound(&msg.message_id.to_string())
            .await
            .unwrap()
            .is_none(),
        "no evidence for an unverified receipt"
    );
}

// ── Inbound ───────────────────────────────────────────────────────────────────

/// A correctly signed interchange from a pinned partner, addressed to us,
/// passes the UNB guards and spawns its process; the partner holds a verified
/// receipt.
#[tokio::test(flavor = "multi_thread")]
async fn an_interchange_from_a_configured_partner_is_routed() {
    use mako_engine::registry::ProcessRegistry as _;
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, true).await;
    let partner = pki.counterparty(&pki.partner, &loopback);

    let receipt = partner
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, OWN),
            "m1",
        ))
        .await
        .expect("receipted");
    assert_eq!(receipt.evidence().message_id(), "m1@partner.test");
    assert_eq!(inbox.log.take(), ["claim", "accept"]);

    let found = inbox
        .store
        .as_process_registry()
        .lookup_correlated(
            mako_engine::ids::TenantId::from_party_id(OWN),
            "51238696781",
        )
        .await
        .unwrap();
    assert!(
        found
            .iter()
            .any(|id| id.workflow_id.name.as_ref() == "gpke-supplier-change"),
        "the interchange spawned its process: {found:?}"
    );
    let kept = As4Evidence::new(inbox.store.clone())
        .inbound(OWN)
        .await
        .unwrap();
    assert_eq!(kept.len(), 1, "the inbound evidence is kept");

    // The retransmission is a duplicate: a fresh receipt, no second dispatch.
    partner
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, OWN),
            "m1",
        ))
        .await
        .expect("a duplicate is receipted");
    assert_eq!(inbox.log.take(), ["claim"]);
}

/// A refused message is durably dead-lettered before its claim is
/// settled, and the refusal is permanent (`EBMS:0004`) — the sender does not
/// retransmit a message that is recorded and could never pass. An interchange
/// addressed to a foreign MP-ID (Allgemeine Festlegungen V6.1d §2.13) is one.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_dead_lettered_before_the_claim_is_released_for_good() {
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, false).await;
    let partner = pki.counterparty(&pki.partner, &loopback);

    let err = partner
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, STRANGER),
            "m2",
        ))
        .await
        .expect_err("refused");
    assert_eq!(err.kind(), asx::ErrorKind::PartnerError);
    assert_eq!(err.ebms_code(), Some("EBMS:0004"));
    assert!(!err.is_retryable(), "a permanent refusal is not retried");
    assert_eq!(inbox.log.take(), ["claim", "dead_letter", "abandon"]);
    assert!(
        inbox.sink.reasons.lock().unwrap()[0].contains("foreign MP-ID"),
        "the dead letter names the refusal"
    );
}

/// A refusal whose dead letter cannot be written leaves no record, so it is
/// retryable (`EBMS:0202`) and the retransmission is processed again.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrecorded_refusal_is_retryable() {
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, false).await;
    let partner = pki.counterparty(&pki.partner, &loopback);
    let message = || {
        push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, STRANGER),
            "m3",
        )
    };

    inbox
        .fail_dead_letters
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let err = partner.send(message()).await.expect_err("refused");
    assert_eq!(err.ebms_code(), Some("EBMS:0202"));
    assert!(err.is_retryable());
    assert_eq!(inbox.log.take(), ["claim", "dead_letter_failed", "abandon"]);

    inbox
        .fail_dead_letters
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let err = partner.send(message()).await.expect_err("refused again");
    assert_eq!(err.ebms_code(), Some("EBMS:0004"));
    assert_eq!(
        inbox.log.take(),
        ["claim", "dead_letter", "abandon"],
        "the retransmission is processed as first-seen and now recorded"
    );
}

/// An interchange that is accepted while a message in it is dead-lettered
/// (unroutable here: no workflow serves it) settles its claim only after the
/// dead letter is written.
#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_interchange_settles_after_its_dead_letters() {
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, false).await;
    let partner = pki.counterparty(&pki.partner, &loopback);

    partner
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, OWN),
            "m4",
        ))
        .await
        .expect("the interchange is receipted");
    assert_eq!(inbox.log.take(), ["claim", "dead_letter", "accept"]);
}

/// `[policy.as4] unbekannte_absender = "nur_konfigurierte"` (the default):
/// a sender whose SM-PKI certificate carries its MP-ID, but who is not a
/// configured partner, is refused before dispatch, retryably.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconfigured_sender_is_refused_under_nur_konfigurierte() {
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, false).await;
    let stranger = pki.counterparty(&pki.stranger, &loopback);

    let err = stranger
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(STRANGER, OWN),
            "m5",
        ))
        .await
        .expect_err("not a configured partner");
    assert_eq!(err.ebms_code(), Some("EBMS:0202"));
    assert_eq!(
        inbox.log.take(),
        ["claim", "abandon"],
        "nothing dispatched, nothing recorded"
    );
}

/// `bdew_ou_zulassen` (BDEW AS4-Profil 1.2 §2.3.2): the same sender is served.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconfigured_sender_is_served_under_bdew_ou_zulassen() {
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::BdewOuZulassen, false).await;
    let stranger = pki.counterparty(&pki.stranger, &loopback);

    stranger
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(STRANGER, OWN),
            "m6",
        ))
        .await
        .expect("served and receipted");
    assert_eq!(inbox.log.take(), ["claim", "dead_letter", "accept"]);
}

/// A request whose signed bytes were altered in transit is refused
/// `EBMS:0101` before any claim is taken.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_request_is_refused_ebms_0101() {
    use tower::Service as _;
    let pki = Pki::new();
    let loopback = Loopback::new();
    let inbox = mount_inbox(&pki, &loopback, UnbekannteAbsender::NurKonfigurierte, false).await;
    let partner = pki.counterparty(&pki.partner, &loopback);
    partner
        .send(push(
            &pki.own.partner(OWN_URL),
            utilmd_55001(PARTNER, OWN),
            "m7",
        ))
        .await
        .expect("the original is receipted");
    inbox.log.take();
    let original = loopback.last_request().expect("the request");

    // Change one character of the signed eb:MessageId and resend the bytes.
    let body = String::from_utf8_lossy(original.body())
        .replacen("m7@partner.test", "m8@partner.test", 1)
        .into_bytes();
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri(OWN_URL)
        .body(http_body_util::Full::new(axum::body::Bytes::from(body)))
        .unwrap();
    *request.headers_mut() = original.headers().clone();

    let mut inbound = {
        let clients = pki.makod(&loopback, UnbekannteAbsender::NurKonfigurierte);
        let state = Arc::new(makod::edifact_api::EdifactApiState {
            platform: Arc::new(edi_energy::Platform::with_all_profiles()),
            pid_router: mako_engine::pid_router::PidRouter::new(),
            mp_id_registry: registry(),
            cedar: Arc::new(makod::cedar_authz::CedarAuthorizer::unauthenticated().unwrap()),
            max_body_bytes: 64 << 20,
            partner_store: None,
            tenant_id: mako_engine::ids::TenantId::from_party_id(OWN),
            dl_sink: Arc::clone(&inbox.sink) as Arc<dyn DeadLetterSink>,
            dispatcher: None,
            contrl_ack: None,
        });
        as4_ingest::inbound(
            clients.client(OWN).unwrap(),
            RecordingDedup {
                inner: SlateDbDedup::new(inbox.store.as_inbox_store()),
                log: Arc::clone(&inbox.log.0),
            },
            Arc::new(BdewAs4IngestHandler::new(state, OWN, clients.clone())),
        )
    };
    let response = inbound.call(request).await.unwrap();
    assert_eq!(response.status(), 400);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("EBMS:0101"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        inbox.log.take().is_empty(),
        "no claim for an unverified message"
    );
}
