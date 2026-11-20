//! One suite per replaceable service.

mod vertragd;

use crate::suite::Suite;

/// Every suite.
pub static SUITES: &[&dyn Suite] = &[&vertragd::Vertragd];

/// The suite of `service`.
#[must_use]
pub fn suite(service: &str) -> Option<&'static dyn Suite> {
    SUITES.iter().copied().find(|s| s.service() == service)
}

#[cfg(test)]
mod tests {
    use super::SUITES;

    /// A suite declares only catalogued types, and every published schema of
    /// its service names one of them.
    #[test]
    fn declared_types_are_catalogued() {
        for s in SUITES {
            for ty in s.emits() {
                assert!(mako_events::all().contains(&ty), "{}: {ty}", s.service());
            }
            assert!(!s.emits().is_empty(), "{} declares no type", s.service());
        }
    }

    /// Every suite has the documents it is checked against.
    #[test]
    fn every_suite_has_its_documents() {
        for s in SUITES {
            let has = |list: &[(&str, &str)]| list.iter().any(|(n, _)| *n == s.service());
            assert!(
                has(crate::documents::OPENAPI),
                "{}: openapi.json",
                s.service()
            );
            assert!(
                has(crate::documents::MCP_SNAPSHOTS),
                "{}: mcp.snapshot.json",
                s.service()
            );
        }
    }
}
