-- VPP Contract Registry — maps SteuerbareRessource-ID → VPP billing configuration.
--
-- A `vpp_contract` record binds a controllable resource (identified by its SR-ID
-- from `marktd`) to the billing parameters needed to auto-settle a WiM
-- Steuerungsauftrag `EndantwortPositiv` (PID 55168) confirmation.
--
-- When `billingd` receives a `de.vpp.dispatch.confirmed` CloudEvent it looks up
-- the active contract for the dispatched SR-ID (matching `valid_from ≤ today` and
-- `valid_to IS NULL OR valid_to > today`) and generates the settlement `Rechnung`
-- automatically.
--
-- Regulatory basis: RED III Article 17 / §41b EnWG (expected 2026).

CREATE TABLE IF NOT EXISTS vpp_contracts (
    id                          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    -- SteuerbareRessource-ID (C…) or NeLo-ID (E…/10Y…) of the controlled resource.
    sr_id                       TEXT         NOT NULL,
    -- Operator-assigned VPP portfolio identifier (free text; used as the
    -- `vpp_id` path parameter on `POST /api/v1/billing/vpp/{vpp_id}`).
    vpp_id                      TEXT         NOT NULL,
    -- MaLo-ID of the VPP aggregation point billed in the settlement invoice.
    malo_id                     TEXT         NOT NULL,
    -- LF/Aggregator MP-ID (invoice issuer).
    lf_mp_id                    TEXT         NOT NULL,
    -- Agreed capacity price in EUR/kWh (Einsatzkosten).
    capacity_price_eur_per_kwh  NUMERIC(12, 6) NOT NULL CHECK (capacity_price_eur_per_kwh >= 0),
    -- Contract validity window.
    valid_from                  DATE         NOT NULL,
    valid_to                    DATE,
    -- MwSt rate override (NULL → use billingd default, typically 0.19).
    mwst_rate_override          NUMERIC(5, 4),
    -- Tenant data-isolation key.
    tenant                      TEXT         NOT NULL,
    updated_at                  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    -- Only one active contract per SR-ID per tenant per start date.
    UNIQUE (sr_id, tenant, valid_from)
);

CREATE INDEX IF NOT EXISTS vpp_contracts_sr_tenant
    ON vpp_contracts (sr_id, tenant, valid_from DESC NULLS LAST);

-- Idempotency table for `de.vpp.dispatch.confirmed` webhook delivery.
-- Prevents double-billing when the outbox retries a delivery.
CREATE TABLE IF NOT EXISTS vpp_dispatch_ledger (
    tx_id      TEXT         NOT NULL,
    tenant     TEXT         NOT NULL,
    -- FK to the billing_records row generated for this dispatch.
    record_id  UUID,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tx_id, tenant)
);
