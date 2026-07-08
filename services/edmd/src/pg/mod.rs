//! PostgreSQL/TimescaleDB repository implementations for `edmd`.

pub mod timeseries;

pub use timeseries::PgTimeSeriesRepository;
