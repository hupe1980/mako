//! The `SG4 IDE+24` Vorgangsnummer a door draws for an outbound UTILMD Vorgang.
//!
//! DE 7402 is `an..35` in the UTILMD MIG, must be unique per Vorgang, and is the
//! number the counterparty's answer names in `SG6 RFF+TN`. A number past the
//! bound is a message the receiver's MIG check refuses — and with it the
//! answer that would have been matched by it.

/// A fresh Vorgangsnummer: the 32 hex digits of a UUID behind `prefix`, which
/// may take at most three characters so the whole stays within `an..35`.
pub(super) fn neu(prefix: &str) -> String {
    debug_assert!(
        prefix.len() <= 3,
        "prefix {prefix:?} pushes DE 7402 past an..35"
    );
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    /// The tightest `an..n` the UTILMD MIGs give DE 7402.
    fn de_7402_max() -> usize {
        let mut max = usize::MAX;
        for mig in [
            include_str!("../../../../../crates/edi-energy/profiles/utilmd/fv20251001/mig.json"),
            include_str!("../../../../../crates/edi-energy/profiles/utilmd/fv20261001/mig.json"),
        ] {
            let v: serde_json::Value = serde_json::from_str(mig).expect("a MIG");
            walk(&v, &mut max);
        }
        assert_ne!(max, usize::MAX, "the MIGs name DE 7402");
        max
    }

    fn walk(v: &serde_json::Value, max: &mut usize) {
        match v {
            serde_json::Value::Object(m) => {
                if m.get("id").and_then(|i| i.as_str()) == Some("7402")
                    && let Some(n) = m
                        .get("format")
                        .and_then(|f| f.as_str())
                        .and_then(|f| f.strip_prefix("an.."))
                        .and_then(|n| n.parse::<usize>().ok())
                {
                    *max = (*max).min(n);
                }
                m.values().for_each(|x| walk(x, max));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, max)),
            _ => {}
        }
    }

    #[test]
    fn every_drawn_vorgangsnummer_fits_de_7402() {
        let max = de_7402_max();
        for prefix in ["", "ANF"] {
            let n = super::neu(prefix);
            assert!(n.len() <= max, "{n:?} is {} > an..{max}", n.len());
        }
    }
}
