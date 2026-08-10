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
    -- SHA-256 of `source`, lowercase hex. The identity of the template.
    hash            TEXT        PRIMARY KEY,
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
    -- whose embedded invoice was extracted again and matched, PARSED means only
    -- that it compiles and exports the contract function. An INVOICE row is
    -- always RENDERED_PDFA; the Textform kinds have no view to render against
    -- yet, and a column saying so beats a comment implying otherwise.
    proof           TEXT        NOT NULL
                        CHECK (proof IN ('RENDERED_PDFA', 'RENDERED_TEXTFORM', 'PARSED')),
    published_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_by    TEXT,
    -- Each kind is stored only with the strongest proof its contract allows:
    -- an invoice renders a conformant PDF/A carrier; a Mahnung has a view and
    -- a specimen since 2026-08, so PARSED is no longer an admissible proof for
    -- it; PREISANPASSUNG still has no view (its data lives in vertragd) and
    -- parses only.
    CONSTRAINT dt_proof_matches_kind CHECK (
        (kind = 'INVOICE'        AND proof = 'RENDERED_PDFA' AND pdf_standard IS NOT NULL)
     OR (kind = 'MAHNUNG'        AND proof = 'RENDERED_TEXTFORM')
     OR (kind = 'PREISANPASSUNG' AND proof = 'PARSED')
    )
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
    hash            TEXT        NOT NULL REFERENCES document_templates(hash),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, kind)
);

COMMENT ON TABLE document_template_current IS
    'Which published template each tenant renders with now. The pointer is '
    'mutable; the templates it references are not.';
