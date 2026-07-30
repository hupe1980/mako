//! Balance-group topology identifiers (MaBiS).
//!
//! `BilanzierungsgebietId` and `BilanzkreisId` model the balance-group topology
//! from BK6-22-024 (MaBiS). MABIS Summenzeitreihen key on them, which is why
//! they live in `mako-mabis`.

use serde::{Deserialize, Serialize};

/// A Bilanzierungsgebiet (settlement zone) within the German electricity grid.
///
/// Each ÜNB / NB operates one or more Bilanzierungsgebiete. All MaLos within a
/// Bilanzierungsgebiet belong to the same settlement pool for MaBiS.
///
/// Source: BK6-22-024 (MaBiS) — Bilanzierungsgebiet definitions; BDEW code list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzierungsgebietId(pub String);

impl std::fmt::Display for BilanzierungsgebietId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Bilanzkreis (balance group) within a Bilanzierungsgebiet.
///
/// A BKV (Bilanzkreisverantwortlicher) holds one or more Bilanzkreise. Each MaLo
/// is assigned to exactly one Bilanzkreis within its Bilanzierungsgebiet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BilanzkreisId(pub String);

impl std::fmt::Display for BilanzkreisId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
