#!/bin/bash
# demos/o2c/init-db.sh — PostgreSQL init for the order-to-cash demo.
#
# One database per service: each owns its schema and its own
# `_sqlx_migrations` table, so two services running migration #1 do not
# collide on a checksum.
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" <<-EOSQL
    CREATE DATABASE productd;
    GRANT ALL PRIVILEGES ON DATABASE productd TO "$POSTGRES_USER";
    CREATE DATABASE vertragd;
    GRANT ALL PRIVILEGES ON DATABASE vertragd TO "$POSTGRES_USER";
    CREATE DATABASE billingd;
    GRANT ALL PRIVILEGES ON DATABASE billingd TO "$POSTGRES_USER";
    CREATE DATABASE outputd;
    GRANT ALL PRIVILEGES ON DATABASE outputd TO "$POSTGRES_USER";
    CREATE DATABASE accountingd;
    GRANT ALL PRIVILEGES ON DATABASE accountingd TO "$POSTGRES_USER";
EOSQL
