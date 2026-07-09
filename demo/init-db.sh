#!/bin/bash
# demo/init-db.sh — PostgreSQL init script run on first container startup.
#
# Creates one database per service following the microservices DB autonomy
# principle: each service owns its schema and migration table in isolation.
# This is required for Helm chart deployments where services may be deployed
# on separate PostgreSQL instances or at different times.
#
# Databases created:
#   marktd    — Market Data Hub (MaLo, MeLo, contracts, subscriptions, …)
#   processd  — Process Decision Engine (anmeldung_decisions, approval_queue)
#   invoicd   — INVOIC plausibility (invoic_receipts, invoic_dlq — §22 MessZV)
#   edmd      — Energy Data Management (meter_reads, billing_periods)
#   obsd      — Business-Process Observability (process_projections)
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
EOSQL
