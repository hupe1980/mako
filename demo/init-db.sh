#!/bin/bash
# demo/init-db.sh — PostgreSQL init script run on first container startup.
#
# Creates one database per service following the microservices DB autonomy
# principle: each service owns its schema and migration table in isolation.
# This is required for Helm chart deployments where services may be deployed
# on separate PostgreSQL instances or at different times.
#
# Databases created:
#   marktd       — Market Data Hub (MaLo, MeLo, contracts, subscriptions, …)
#   processd     — Process Decision Engine (anmeldung_decisions, approval_queue)
#   invoicd      — INVOIC plausibility (invoic_receipts, invoic_dlq — §22 MessZV)
#   edmd         — Energy Data Management (meter_reads, billing_periods)
#   obsd         — Business-Process Observability (process_projections)
#   netzbilanzd  — NNE/MMM billing daemon (invoice_drafts)
#   sperrd       — Sperrung execution tracking (sperr_orders)
#   einsd        — Einspeiser Registry + EEG/KWKG Settlement
#   tarifbd      — Product & Tariff Catalog (products, customer_products, epex_prices)
#   billingd     — Energy Billing Engine (billing_records, billing_run_log)
#   accountingd  — Customer Account Ledger (accounts, ledger_entries, sepa_mandates)
#   vertragd     — Contract & Customer Management (kunden, kunden_identitaeten, vertraege)
#
# nis-syncd, portald, agentd are stateless or use non-PostgreSQL stores — no database needed.
#
# SQLx note: each service runs `sqlx::migrate!()` against its own database,
# so there is no `_sqlx_migrations` checksum conflict between services.
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" <<-EOSQL
    CREATE DATABASE marktd;
    GRANT ALL PRIVILEGES ON DATABASE marktd TO "$POSTGRES_USER";
    CREATE DATABASE processd;
    GRANT ALL PRIVILEGES ON DATABASE processd TO "$POSTGRES_USER";
    CREATE DATABASE invoicd;
    GRANT ALL PRIVILEGES ON DATABASE invoicd TO "$POSTGRES_USER";
    CREATE DATABASE edmd;
    GRANT ALL PRIVILEGES ON DATABASE edmd TO "$POSTGRES_USER";
    CREATE DATABASE obsd;
    GRANT ALL PRIVILEGES ON DATABASE obsd TO "$POSTGRES_USER";
    CREATE DATABASE netzbilanzd;
    GRANT ALL PRIVILEGES ON DATABASE netzbilanzd TO "$POSTGRES_USER";
    CREATE DATABASE sperrd;
    GRANT ALL PRIVILEGES ON DATABASE sperrd TO "$POSTGRES_USER";
    CREATE DATABASE einsd;
    GRANT ALL PRIVILEGES ON DATABASE einsd TO "$POSTGRES_USER";
    CREATE DATABASE tarifbd;
    GRANT ALL PRIVILEGES ON DATABASE tarifbd TO "$POSTGRES_USER";
    CREATE DATABASE billingd;
    GRANT ALL PRIVILEGES ON DATABASE billingd TO "$POSTGRES_USER";
    CREATE DATABASE accountingd;
    GRANT ALL PRIVILEGES ON DATABASE accountingd TO "$POSTGRES_USER";
    CREATE DATABASE vertragd;
    GRANT ALL PRIVILEGES ON DATABASE vertragd TO "$POSTGRES_USER";
EOSQL
