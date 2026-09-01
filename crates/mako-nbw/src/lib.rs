//! `mako-nbw` — Netzbetreiberwechsel (DSO concession change) process engine.
//!
//! **This crate is a name reservation. Implementation is pending.**
//!
//! Netzbetreiberwechsel is the § 46 EnWG handover of every Marktlokation in a
//! grid area from the outgoing to the incoming Netzbetreiber when a municipal
//! concession changes hands. Unlike every other MaKo process it is a bulk
//! migration — thousands of MaLo in one event, over months — rather than an
//! event-driven per-message workflow, which is why it gets its own crate rather
//! than a module in `mako-gpke`.
//!
//! The PARTIN PIDs it will carry (37000–37014) are already routed as day-to-day
//! Kommunikationsdaten by `mako-gpke` (`gpke-partin`, Strom) and
//! `mako-geli-gas` (`geli-gas-partin`, Gas); this crate reserves the bulk
//! context, both Sparten in one place. See [`README.md`] for the PID inventory,
//! the market roles and the regulatory sources.
//!
//! [`README.md`]: https://github.com/hupe1980/mako/blob/main/crates/mako-nbw/README.md

#![deny(unsafe_code)]
#![deny(missing_docs)]
