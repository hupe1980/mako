-- One PostgreSQL for the whole estate, a database per service: each owns its
-- schema and its own `_sqlx_migrations`, so two services applying a first
-- migration never collide on a checksum. Run once, on an empty data
-- directory, by the image's entrypoint as `POSTGRES_USER` (`mako`), which owns
-- every database it creates here.
--
-- Used by `just dev` (deploy/dev/compose.yaml) and `just demo` (compose.yaml).
-- The passwords are development literals; a deployment sets its own.
CREATE DATABASE marktd;
CREATE DATABASE processd;
CREATE DATABASE invoicd;
CREATE DATABASE edmd;
CREATE DATABASE netzbilanzd;
CREATE DATABASE einsd;
CREATE DATABASE productd;
CREATE DATABASE billingd;
CREATE DATABASE outputd;
CREATE DATABASE accountingd;
CREATE DATABASE vertragd;
CREATE DATABASE agentd;

-- accountingd serves as a role that may append to its journal and not
-- rewrite it; the owner above migrates and grants it (`[database]
-- migration_url`).
CREATE ROLE accountingd_app LOGIN PASSWORD 'accountingd_app';
GRANT CONNECT ON DATABASE accountingd TO accountingd_app;
