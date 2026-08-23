-- outputd — customer-document rendering.
--
-- outputd owns *how documents look*: operator-published Typst templates and
-- the render engine that turns a caller's view into a PDF (for invoices, a
-- ZUGFeRD PDF/A-3 carrier around the caller's CII payload). What a document
-- *says* — amounts, VAT, legal basis — stays with the issuing service
-- (billingd for invoices and Mahnungen); outputd never recomputes it.

-- ── Document templates ───────────────────────────────────────────────────────
--
-- The operator owns the *visual* layout of a document (logo, Briefkopf, where
-- the Pflichtangaben sit); an invoice's embedded CII XML is always rendered by
-- the caller from the EN 16931 semantic model, never from a template. See
-- `outputd::document`.
--
-- **Content-addressed and append-only.** An invoice is a Buchungsbeleg kept for
-- 8 years (§ 14b UStG / § 147 AO), and GoBD requires Unveraenderbarkeit — a
-- document issued today must still be explicable in 2034. A mutable template row
-- would silently rewrite the history of how documents looked, so a template is
-- identified by the hash of its source and never updated in place. Publishing a
-- change means inserting a new row and moving the pointer.
--
-- Issuing services record the hash outputd returns (`X-Mako-Template-Hash`)
-- next to the document they issued. That pin lives in *their* database, so no
-- foreign key can guard it — this table's append-only policy is what keeps
-- those pins resolvable. Never UPDATE or DELETE here.

CREATE TABLE document_templates (
    -- SHA-256 of `source`, lowercase hex. Together with the tenant, the
    -- identity of the template: content-addressing is scoped to the operator
    -- who published it. A globally unique hash would let one tenant's row
    -- occupy an identity every other tenant computes for the same bytes — and
    -- since outputd ships a reference layout operators are told to start from,
    -- the very first tenant to publish it unchanged would lock all the others
    -- out of it, and the refusal would disclose that some other tenant had
    -- published that exact source.
    hash            TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- Which document this renders. Textform kinds share the engine and the
    -- store with the invoice kind so an operator maintains one template system.
    kind            TEXT        NOT NULL CHECK (kind IN (
                        'INVOICE',          -- ZUGFeRD PDF/A-3 carrier
                        'MAHNUNG',          -- Textform (§ 126b BGB)
                        'PREISANPASSUNG'    -- § 41 Abs. 5 EnWG notice, Textform
                    )),
    -- The template source. Typst.
    source          TEXT        NOT NULL,
    -- PDF/A conformance level the publish gate enforced, in Typst's spelling
    -- (`a-3b`). NULL for the Textform kinds, which have no PDF/A to meet.
    pdf_standard    TEXT,
    -- What the publish gate actually established about this template. Recorded
    -- rather than assumed: RENDERED_PDFA means it produced a conformant carrier
    -- whose embedded invoice was extracted again and matched;
    -- RENDERED_TEXTFORM means it rendered the kind's specimen and the page
    -- carried the content the statute makes part of the notice.
    --
    -- There is no weaker level. A `PARSED` one existed — the template compiles
    -- and exports `render` — for kinds whose data contract had not been
    -- projected into a view, and it established nothing about the page: a
    -- PREISANPASSUNG template could be rolled out while printing none of what
    -- § 41 Abs. 5 EnWG requires, including the Sonderkündigungsrecht without
    -- which the Anzeige is invalid. Every kind now has a specimen.
    proof           TEXT        NOT NULL
                        CHECK (proof IN ('RENDERED_PDFA', 'RENDERED_TEXTFORM')),
    published_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_by    TEXT,
    -- Each kind is stored only with the proof its contract requires: an invoice
    -- renders a conformant PDF/A carrier, a Textform kind renders its specimen.
    CONSTRAINT dt_proof_matches_kind CHECK (
        (kind = 'INVOICE'        AND proof = 'RENDERED_PDFA' AND pdf_standard IS NOT NULL)
     OR (kind IN ('MAHNUNG', 'PREISANPASSUNG') AND proof = 'RENDERED_TEXTFORM')
    ),
    PRIMARY KEY (tenant, hash)
);

COMMENT ON TABLE document_templates IS
    'Append-only, content-addressed store of operator document templates. '
    'Never UPDATE or DELETE: an issued document pins the hash that rendered it, '
    'and § 147 AO / GoBD require that to stay resolvable for 8 years.';

-- One template per (tenant, kind) is "the current one". This pointer moves;
-- the rows it points at do not.
CREATE TABLE document_template_current (
    tenant          TEXT        NOT NULL,
    kind            TEXT        NOT NULL,
    hash            TEXT        NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, kind),
    -- Composite, because a hash alone no longer names a row: the pointer may
    -- only reference a template this same tenant published.
    FOREIGN KEY (tenant, hash) REFERENCES document_templates (tenant, hash)
);

COMMENT ON TABLE document_template_current IS
    'Which published template each tenant renders with now. The pointer is '
    'mutable; the templates it references are not.';

-- ── Issued documents ─────────────────────────────────────────────────────────
--
-- What was actually **sent to a customer**, as opposed to what a template can
-- render. `POST /render` produces bytes and keeps none; that is enough to make
-- a document and not to communicate one. Two things rest on this table:
--
--   * **§ 14 Abs. 1 Satz 2 UStG / § 147 Abs. 1 Nr. 2–3 AO** — the
--     Rechnungsdoppel is kept eight years *in the form in which it was issued*
--     (GoBD: Unveränderbarkeit). A pinned template hash makes the layout
--     resolvable; it does not make the document resolvable, because the data
--     behind it moves.
--   * **§ 126b BGB, § 41f Abs. 1 und Abs. 5 EnWG** — the disconnection
--     sequence runs on notices that must have *reached* the customer, four
--     weeks and eight Werktage before the act.
--
-- Append-only, like `document_templates` and for the same statute. Never UPDATE
-- the content columns; a corrected document is a new row.

CREATE TABLE documents (
    document_id     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    kind            TEXT        NOT NULL CHECK (kind IN (
                        'INVOICE', 'MAHNUNG', 'PREISANPASSUNG'
                    )),
    -- The template that rendered it. A plain value rather than a foreign key:
    -- it is the same pin issuing services keep, and `document_templates` is
    -- append-only so it stays resolvable without one.
    template_hash   TEXT        NOT NULL,

    -- ── Who it is about ──────────────────────────────────────────────────────
    -- `subject_ref` is the thing this document documents, in the issuing
    -- service's own terms: a Rechnungsnummer for an INVOICE, a
    -- `dunning_cases.id` for a MAHNUNG, a slice id for a PREISANPASSUNG.
    -- Unique per (tenant, kind), so a service that retries a render cannot end
    -- up having sent the same notice twice.
    subject_ref     TEXT        NOT NULL,
    malo_id         TEXT,
    kunden_nr       TEXT,

    -- ── The bytes ────────────────────────────────────────────────────────────
    -- Stored, not re-rendered on demand. A re-render is not a reproduction —
    -- the rolled-out template moves, the operator's address changes, the
    -- renderer is upgraded — and § 147 AO asks for the document as issued.
    content         BYTEA       NOT NULL,
    content_sha256  TEXT        NOT NULL,
    byte_size       INTEGER     NOT NULL CHECK (byte_size > 0),
    media_type      TEXT        NOT NULL DEFAULT 'application/pdf',

    -- ── Recipient, as addressed at issue time ────────────────────────────────
    -- Snapshotted: the customer's address is live master data in vertragd, and
    -- what matters afterwards is where the notice was actually sent.
    recipient_name  TEXT,
    recipient_email TEXT,
    recipient_address JSONB,

    issued_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    issued_by       TEXT,

    UNIQUE (tenant, kind, subject_ref)
);

COMMENT ON TABLE documents IS
    'Append-only store of documents actually issued to customers, with the bytes '
    'that were sent. § 14 Abs. 1 UStG / § 147 AO (8 years, unveraendert) and the '
    '§ 41f EnWG notice evidence both rest on it. Never UPDATE content.';

CREATE INDEX doc_subject ON documents (tenant, kind, subject_ref);
CREATE INDEX doc_malo    ON documents (tenant, malo_id, issued_at DESC) WHERE malo_id IS NOT NULL;
CREATE INDEX doc_kunde   ON documents (tenant, kunden_nr, issued_at DESC) WHERE kunden_nr IS NOT NULL;

-- ── Delivery attempts and their evidence ─────────────────────────────────────
--
-- One row per (document, channel). A document may go out on several at once —
-- portal inbox plus e-mail is the usual pair — and each carries its own
-- outcome, because they fail independently.
--
-- **What counts as delivered differs by channel:**
--
--   * `PORTAL` — `DELIVERED` once published: the document is then in the
--     recipient's sphere, which is what § 126b BGB asks of a durable medium.
--     `read_at` records that they opened it — more than Textform requires, and
--     what a dispute asks about.
--   * `EMAIL` — `SENT` when the relay accepted it, `DELIVERED` only when the
--     relay reports the recipient's server did. An accepted hand-off followed
--     by a bounce is a notice that never arrived.
--   * `POST` — `SENT` when the print service collects it, `DELIVERED` when it
--     reports it posted. Without registered mail there is no further evidence,
--     and the schema invents none.
--   * `ERP` — handed to the operator's own system, which then owns delivery.
--
-- `FAILED` is terminal at the configured attempt ceiling. `SUPPRESSED` is a
-- channel that was never viable (no e-mail address on file), recorded rather
-- than skipped so "why did this never go out" is answerable.

CREATE TABLE document_deliveries (
    delivery_id     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id     UUID        NOT NULL REFERENCES documents (document_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    channel         TEXT        NOT NULL CHECK (channel IN ('PORTAL', 'EMAIL', 'POST', 'ERP')),
    status          TEXT        NOT NULL DEFAULT 'PENDING' CHECK (status IN (
                        'PENDING', 'SENT', 'DELIVERED', 'FAILED', 'SUPPRESSED'
                    )),
    -- Where it went, in the channel's own terms: an e-mail address, a postal
    -- address. NULL for a SUPPRESSED row — its absence is usually the reason.
    target          TEXT,
    attempts        INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    -- When the worker may next try. Here rather than in worker memory, so a
    -- restart does not retry everything at once.
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    first_sent_at   TIMESTAMPTZ,
    delivered_at    TIMESTAMPTZ,
    read_at         TIMESTAMPTZ,
    -- The channel's own receipt — message id, batch reference, relay response.
    -- What an operator shows when asked to prove a § 41f notice went out.
    evidence        JSONB,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- One attempt track per channel per document: a retry advances this row
    -- rather than inserting a second, so `attempts` is the whole history.
    UNIQUE (document_id, channel),
    -- A delivered row states when. A pending one cannot claim to have been.
    CHECK ((status = 'DELIVERED') = (delivered_at IS NOT NULL)),
    CHECK (status <> 'SUPPRESSED' OR last_error IS NOT NULL)
);

COMMENT ON TABLE document_deliveries IS
    'Per-document, per-channel delivery attempts and their evidence. What '
    '"delivered" means differs by channel. Backs the § 126b BGB Textform and '
    '§ 41f EnWG notice evidence.';

-- The worker's claim scan.
CREATE INDEX dd_due ON document_deliveries (tenant, next_attempt_at)
    WHERE status = 'PENDING';
-- The print service's spool pull.
CREATE INDEX dd_spool ON document_deliveries (tenant, created_at)
    WHERE channel = 'POST' AND status = 'PENDING';
CREATE INDEX dd_document ON document_deliveries (document_id);
