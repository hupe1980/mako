//! Gas quality normalisation for the GeLi Gas / WiM Gas process family.
//!
//! ## The vocabulary
//!
//! BO4E `Gasqualitaet` has exactly two values, `H_GAS` and `L_GAS` (DVGW G 260
//! §3.2, by Wobbe index — H-Gas 12.4–15.7 kWh/m³, L-Gas 10.5–13.0 kWh/m³), and
//! `marktd.malo.gasqualitaet` is `CHECK`-constrained to them. Those two are the
//! only values this module produces.
//!
//! No H2-blend spelling is guessed ahead of the 2026–2028 DVGW/BNetzA wave:
//! BO4E's own schema documentation says not to, because rows persisted under a
//! guessed string stay wrong once the standard lands with a different one.
//! Adopting one is a BO4E schema bump plus an AHB code, and the mapping belongs
//! here at that point.
//!
//! ## The aliases
//!
//! Older UTILMD G payloads and operator ERP exports spell the two real
//! qualities several ways (`HGas`, `H-Gas`, `ERDGAS_H`). Normalising them is
//! this module's job; an unrecognised value returns `None` so the caller leaves
//! the stored column untouched.

/// Normalise a raw `gasqualitaet` string to its BO4E `Gasqualitaet` wire value.
///
/// Case-insensitive, whitespace-trimmed, and tolerant of `-`/space separators.
/// Returns `None` for anything that is not one of the two qualities BO4E
/// defines — including a future H2 blend, which has no sanctioned spelling yet.
///
/// ## Examples
///
/// ```rust
/// use mako_geli_gas::gas_quality::normalize_gasqualitaet;
///
/// assert_eq!(normalize_gasqualitaet("HGas"), Some("H_GAS"));
/// assert_eq!(normalize_gasqualitaet("L-Gas"), Some("L_GAS"));
/// assert_eq!(normalize_gasqualitaet("  H_GAS  "), Some("H_GAS")); // idempotent
/// assert_eq!(normalize_gasqualitaet("H2_BLEND"), None); // not in BO4E v202607
/// ```
#[must_use]
pub fn normalize_gasqualitaet(raw: &str) -> Option<&'static str> {
    let norm = raw.trim().to_uppercase().replace(['-', ' '], "_");
    match norm.as_str() {
        "HGAS" | "H_GAS" | "HIGH_CALORIFIC" | "HOCHKALORISCH" | "ERDGAS_H" => Some("H_GAS"),
        "LGAS" | "L_GAS" | "LOW_CALORIFIC" | "NIEDERKALORISCH" | "ERDGAS_L" => Some("L_GAS"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_gasqualitaet;

    #[test]
    fn hgas_aliases_normalize_to_h_gas() {
        for raw in [
            "HGas",
            "H-Gas",
            "H-gas",
            "HGAS",
            "H_GAS",
            "HIGH_CALORIFIC",
            "ERDGAS_H",
            "hochkalorisch",
        ] {
            assert_eq!(normalize_gasqualitaet(raw), Some("H_GAS"), "for {raw:?}");
        }
    }

    #[test]
    fn lgas_aliases_normalize_to_l_gas() {
        for raw in [
            "LGas",
            "L-Gas",
            "L_GAS",
            "LGAS",
            "LOW_CALORIFIC",
            "ERDGAS_L",
        ] {
            assert_eq!(normalize_gasqualitaet(raw), Some("L_GAS"), "for {raw:?}");
        }
    }

    /// An unrecognised quality leaves the column alone rather than becoming a
    /// placeholder: `marktd.malo.gasqualitaet` is `CHECK`ed against the BO4E
    /// vocabulary, so `"UNKNOWN"` would fail the insert.
    #[test]
    fn an_unrecognised_quality_is_none() {
        for raw in ["SYNGAS", "", "H2_BLEND", "BIOGAS", "LPG"] {
            assert_eq!(normalize_gasqualitaet(raw), None, "for {raw:?}");
        }
    }

    /// Whatever this emits must be a value BO4E defines, or the column it feeds
    /// would reject it.
    #[test]
    fn emitted_values_are_bo4e_wire_values() {
        // BO4E v202607 `Gasqualitaet::VARIANTS`. Spelled out rather than
        // imported so this crate stays rubo4e-free (see BO4E_COVERAGE.md §4);
        // `mako-markt`'s CHECK-drift test is what pins the list to the schema.
        const BO4E_GASQUALITAET: [&str; 2] = ["H_GAS", "L_GAS"];
        for raw in ["HGas", "L-Gas", "ERDGAS_H", "NIEDERKALORISCH"] {
            let v = normalize_gasqualitaet(raw).expect("recognised");
            assert!(BO4E_GASQUALITAET.contains(&v), "{raw:?} → {v}");
        }
    }
}
