#!/bin/bash
# demo/init-db.sh — PostgreSQL init for the mako demo stack.
# Creates one database per service. Extra databases cost nothing and allow
# adding optional services without needing to recreate the volume.
# marktd and processd run sqlx::migrate!() on startup — no manual DDL needed.
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
    -- agentd is optional in this demo (needs an external LLM API key);
    -- the database is pre-created so it can be started alongside if desired.
    CREATE DATABASE agentd;
    GRANT ALL PRIVILEGES ON DATABASE agentd TO "$POSTGRES_USER";
EOSQL
