//! PostgreSQL repository implementations for `edmd`.

pub mod timeseries;
pub mod typ2;

pub use timeseries::PgTimeSeriesRepository;
pub use typ2::PgTyp2Repository;
