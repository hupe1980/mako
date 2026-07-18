#!/bin/bash
# demos/eeg-billing/init-db.sh — PostgreSQL init for the EEG billing demo.
# Creates one database per service (microservices DB autonomy principle).
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" <<-EOSQL
    CREATE DATABASE marktd;
    GRANT ALL PRIVILEGES ON DATABASE marktd TO "$POSTGRES_USER";
    CREATE DATABASE edmd;
    GRANT ALL PRIVILEGES ON DATABASE edmd TO "$POSTGRES_USER";
    CREATE DATABASE einsd;
    GRANT ALL PRIVILEGES ON DATABASE einsd TO "$POSTGRES_USER";
EOSQL
