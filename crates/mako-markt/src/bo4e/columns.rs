//! The BO4E → typed-column boundary for Stammdaten rows.
//!
//! marktd stores every Stammdaten object twice: the full BO4E payload in a
//! `data JSONB` column, and a handful of fields in typed columns for querying.
//! This module owns the second half.
//!
//! Three rules hold, and each is enforced rather than documented:
//!
//! * **Columns are derived from the typed BO**, not from string lookups on its
//!   JSON — a field that moves stops compiling.
//! * **A column may only shadow a field the BO declares.** Anything else is
//!   owned by whichever writer owns it, and an upsert leaves it alone.
//! * **A column holds a BO4E wire value and nothing else.** The value comes
//!   from the enum's `as_wire()`; the column's SQL `CHECK` is the enum's
//!   `VARIANTS`, pinned by `bo4e_check_constraints_match_the_schema`.

use rubo4e::current::Marktlokation;

/// The `malo` row's typed columns, derived from the BO4E `Marktlokation`.
///
/// Every field is `Option`: BO4E's schema makes every field optional, and a
/// mako profile at the API boundary decides which ones an endpoint requires.
///
/// Only fields `Marktlokation` declares appear here. `fallgruppe` (a
/// `Bilanzierung` field) and `fernsteuerbar` (no BO4E field) are owned by
/// [`MaloStammdatenPatch`](crate::repository::MaloStammdatenPatch); an upsert
/// leaves those columns alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaloShadowColumns {
    /// `Marktlokation.netzebene` — BO4E `Netzebene` wire value
    /// (`NSP`/`MSP`/`HSP`/`HSS`/`…_UMSP` Strom, `HD`/`MD`/`ND` Gas).
    pub netzebene: Option<&'static str>,
    /// `Marktlokation.bilanzierungsgebiet` — Bilanzierungsgebiet EIC.
    pub bilanzierungsgebiet: Option<String>,
    /// `Marktlokation.gasqualitaet` — BO4E `Gasqualitaet` (`H_GAS`/`L_GAS`).
    pub gasqualitaet: Option<&'static str>,
    /// `Marktlokation.energierichtung` — BO4E `Energierichtung`.
    ///
    /// `EINSP` is the **generating** location (Einspeisung: it feeds the grid)
    /// and `AUSSP` the **consuming** one (Ausspeisung: it draws from the grid).
    /// The direction is named from the grid's point of view.
    pub energierichtung: Option<&'static str>,
    /// `Marktlokation.bilanzierungsmethode` — BO4E `Bilanzierungsmethode`.
    pub bilanzierungsmethode: Option<&'static str>,
    /// `Marktlokation.regelzone` — Regelzone EIC (ÜNB assignment).
    pub regelzone: Option<String>,
    /// `Marktlokation.lokationsbuendelObjektcode`.
    pub lokationsbuendel_objektcode: Option<String>,
}

impl MaloShadowColumns {
    /// Derive the typed columns from a validated `Marktlokation`.
    #[must_use]
    pub fn from_marktlokation(malo: &Marktlokation) -> Self {
        Self {
            netzebene: malo.netzebene.map(|v| v.as_wire()),
            bilanzierungsgebiet: malo.bilanzierungsgebiet.clone(),
            gasqualitaet: malo.gasqualitaet.map(|v| v.as_wire()),
            energierichtung: malo.energierichtung.map(|v| v.as_wire()),
            bilanzierungsmethode: malo.bilanzierungsmethode.map(|v| v.as_wire()),
            regelzone: malo.regelzone.as_ref().map(ToString::to_string),
            lokationsbuendel_objektcode: malo.lokationsbuendel_objektcode.clone(),
        }
    }
}

/// The `melo` row's typed columns.
///
/// [`Standorteigenschaften`](rubo4e::current::Standorteigenschaften) is a
/// standalone BO (#25), not a `Messlokation` field, so it arrives in the
/// extension map where typed deserialization does not reach.
/// [`MeloShadowColumns::from_messlokation`] parses it as the BO it names and
/// reads the EIC off the typed value, so a malformed one is a rejected write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeloShadowColumns {
    /// `Messlokation.netzebeneMessung` — BO4E `Netzebene` wire value.
    pub netzebene_messung: Option<&'static str>,
    /// `Messlokation.lokationsbuendelObjektcode`.
    pub lokationsbuendel_objektcode: Option<String>,
    /// Regelzone EIC from `standorteigenschaften.eigenschaftenStrom[0].regelzone`.
    pub regelzone: Option<String>,
    /// The `Standorteigenschaften` extension, re-serialised from its typed form.
    pub standorteigenschaften: Option<serde_json::Value>,
}

/// Why a `Messlokation` payload's `standorteigenschaften` extension could not be
/// read as the BO it names.
#[derive(Debug, thiserror::Error)]
#[error("`standorteigenschaften` is not a valid BO4E Standorteigenschaften: {0}")]
pub struct StandorteigenschaftenError(String);

impl MeloShadowColumns {
    /// Derive the typed columns from a validated `Messlokation`.
    ///
    /// # Errors
    ///
    /// Returns [`StandorteigenschaftenError`] when the payload carries a
    /// `standorteigenschaften` key that does not parse as the BO4E BO, or whose
    /// enum values are not in the schema.
    pub fn from_messlokation(
        melo: &rubo4e::current::Messlokation,
    ) -> Result<Self, StandorteigenschaftenError> {
        use rubo4e::current::Standorteigenschaften;
        use rubo4e::json::Bo4eExtensionData as _;

        let (regelzone, standorteigenschaften) =
            match melo.extension_data().get("standorteigenschaften") {
                None => (None, None),
                Some(raw) => {
                    // The same gate every BO4E payload crosses, minus the `_typ`
                    // stage: the key in the extension map already named the BO,
                    // and producers do not reliably stamp `_typ` on a nested one.
                    let typed: Standorteigenschaften = super::gate::decode_nested(raw.clone())
                        .map_err(|e| StandorteigenschaftenError(e.to_string()))?;
                    // `regelzoneEic`, not `regelzone`. BO4E ships both and they
                    // are different things: `regelzone` is „Der Name der
                    // Regelzone", `regelzoneEic` is „De EIC-Nummer der
                    // Regelzone". Both render through `ToString`, so reading the
                    // wrong one compiles — and this column is an EIC, indexed
                    // and filtered on to map a MeLo to its ÜNB for MaBiS IFTSTA
                    // and Redispatch 2.0.
                    let eic = typed
                        .eigenschaften_strom
                        .as_ref()
                        .and_then(|v| v.first())
                        .and_then(|s| s.regelzone_eic.as_ref())
                        .map(ToString::to_string);
                    let json = serde_json::to_value(&typed)
                        .map_err(|e| StandorteigenschaftenError(e.to_string()))?;
                    (eic, Some(json))
                }
            };

        Ok(Self {
            netzebene_messung: melo.netzebene_messung.map(|v| v.as_wire()),
            lokationsbuendel_objektcode: melo.lokationsbuendel_objektcode.clone(),
            regelzone,
            standorteigenschaften,
        })
    }
}

/// The `zaehler` row's typed columns.
///
/// Both were envelope fields the caller supplied *beside* the BO4E payload, so
/// nothing stopped `zaehler_typ` from contradicting `data.zaehlertyp`, or
/// `eichung_bis` from naming a different calibration expiry than the document
/// it shadows. `Zaehler` declares both fields, so there is no reason to ask for
/// them twice and every reason not to: a `MessEV` calibration expiry that
/// disagrees with the meter record drives the wrong replacement workflow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZaehlerShadowColumns {
    /// `Zaehler.zaehlertyp` — BO4E `Zaehlertyp` wire value.
    pub zaehler_typ: Option<&'static str>,
    /// `Zaehler.eichungBis`, as a calendar date.
    pub eichung_bis: Option<time::Date>,
}

impl ZaehlerShadowColumns {
    /// Derive the typed columns from a validated `Zaehler`.
    #[must_use]
    pub fn from_zaehler(z: &rubo4e::current::Zaehler) -> Self {
        Self {
            zaehler_typ: z.zaehlertyp.map(|v| v.as_wire()),
            eichung_bis: z.eichung_bis.map(time::OffsetDateTime::date),
        }
    }
}

/// The `geraet` row's typed column, derived from `Geraet.geraetetyp`.
#[must_use]
pub fn geraet_typ(g: &rubo4e::current::Geraet) -> Option<&'static str> {
    g.geraetetyp.map(|v| v.as_wire())
}

/// The `ZusatzAttribut.name` under which a mako-only price type travels.
///
/// BO4E `Preistyp` defines ten values; mako prices things the standard does not
/// model (EEG-Marktprämie, HEMS optimisation events, E-Mobility roaming). Those
/// travel here, with `preistyp` left absent — which the schema permits, every
/// BO4E field being optional:
///
/// ```json
/// {
///   "preisstaffeln": [{ "preis": "8.20" }],
///   "zusatzAttribute": [{ "name": "mako:preistyp", "wert": "EEG_MARKTPRAEMIE" }]
/// }
/// ```
///
/// Writing a mako value into BO4E's own enum field makes a document that
/// `rubo4e` decodes to `Unknown` in silence, and that go-bo4e (`invalid
/// <Enum> %q`, no catch-all variant) and BO4E-python (a pydantic
/// `ValidationError`) refuse outright. The lenient reading is the mild case.
pub const MAKO_PREISTYP_ATTRIBUT: &str = "mako:preistyp";

/// Read the effective price type of one `tarifpreispositionen` entry.
///
/// Checks the BO4E `preistyp` first, then the [`MAKO_PREISTYP_ATTRIBUT`]
/// `ZusatzAttribut`. Returns `""` when neither is present.
#[must_use]
pub fn position_preistyp(position: &serde_json::Value) -> &str {
    if let Some(pt) = position.get("preistyp").and_then(|v| v.as_str())
        && !pt.is_empty()
    {
        return pt;
    }
    position
        .get("zusatzAttribute")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(MAKO_PREISTYP_ATTRIBUT))
        .and_then(|a| a.get("wert"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Whether `preistyp` is one of the ten values BO4E v202607 defines.
///
/// A value outside this set is a mako extension and belongs in the
/// [`MAKO_PREISTYP_ATTRIBUT`] `ZusatzAttribut`, not in the BO4E field.
#[must_use]
pub fn is_bo4e_preistyp(value: &str) -> bool {
    rubo4e::current::Preistyp::from_wire(value).is_ok()
}

/// The `malo` columns whose SQL `CHECK` list must equal a BO4E enum's
/// `VARIANTS`, as `(column, wire values)`.
///
/// `zaehler.zaehler_typ` has its own guard in
/// `services/marktd/tests/schema_enum_guard.rs`, which also pins the
/// `INTELLIGENTES_MESSSYSTEM` spelling `Zaehlertyp` and `Geraetetyp` disagree
/// on. `geraet.geraet_typ` is not `CHECK`-constrained: 48 variants that turn
/// over between BO4E versions would make an inline list the next thing to
/// drift.
///
/// Exposed rather than inlined in the test so a migration generator or an
/// operator script can render the same list instead of re-typing it.
#[must_use]
pub fn malo_enum_check_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    use rubo4e::current::{
        Bilanzierungsmethode, Energierichtung, Fallgruppenzuordnung, Gasqualitaet, Netzebene,
    };
    fn wires<T: rubo4e::Bo4eEnum + 'static>() -> Vec<&'static str> {
        T::VARIANTS.iter().map(rubo4e::Bo4eEnum::as_wire).collect()
    }
    vec![
        ("netzebene", wires::<Netzebene>()),
        ("gasqualitaet", wires::<Gasqualitaet>()),
        ("energierichtung", wires::<Energierichtung>()),
        ("bilanzierungsmethode", wires::<Bilanzierungsmethode>()),
        ("fallgruppe", wires::<Fallgruppenzuordnung>()),
    ]
}

/// The `melo` columns whose SQL `CHECK` list must equal a BO4E enum's
/// `VARIANTS`, as `(column, wire values)`.
#[must_use]
pub fn melo_enum_check_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![("netzebene_messung", netzebene_wires())]
}

/// The `nelo` columns whose SQL `CHECK` list must equal a BO4E enum's
/// `VARIANTS`, as `(column, wire values)`.
///
/// `Netzlokation` declares no `netzebene` field, so this column is mako's own —
/// but the UTILMD Stammdatenänderung patch is a single map routed by object
/// type, so a `NeLo` and a `MaLo` receive the same `netzebene` value. Its
/// vocabulary therefore has to be the schema's.
#[must_use]
pub fn nelo_enum_check_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![("netzebene", netzebene_wires())]
}

/// The `partners` columns whose SQL `CHECK` list must equal a BO4E enum's
/// `VARIANTS`, as `(column, wire values)`.
///
/// Both are served verbatim in `GET /partners/{id}/marktteilnehmer`, so an
/// unconstrained column is a `Marktteilnehmer` the counterparty cannot read.
#[must_use]
pub fn partner_enum_check_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    use rubo4e::current::{Marktrolle, Rollencodetyp};
    vec![
        (
            "marktrolle",
            Marktrolle::VARIANTS
                .iter()
                .map(rubo4e::Bo4eEnum::as_wire)
                .collect(),
        ),
        (
            "rollencodetyp",
            Rollencodetyp::VARIANTS
                .iter()
                .map(rubo4e::Bo4eEnum::as_wire)
                .collect(),
        ),
    ]
}

fn netzebene_wires() -> Vec<&'static str> {
    rubo4e::current::Netzebene::VARIANTS
        .iter()
        .map(rubo4e::Bo4eEnum::as_wire)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::MaloShadowColumns;
    use rubo4e::current::{
        Bilanzierungsmethode, Energierichtung, Gasqualitaet, Marktlokation, Netzebene,
    };

    /// The columns carry BO4E wire values, not prose and not a second spelling.
    #[test]
    fn columns_are_bo4e_wire_values() {
        let malo = Marktlokation {
            netzebene: Some(Netzebene::MspNspUmsp),
            gasqualitaet: Some(Gasqualitaet::LGas),
            energierichtung: Some(Energierichtung::Einsp),
            bilanzierungsmethode: Some(Bilanzierungsmethode::Rlm),
            ..Default::default()
        };
        let cols = MaloShadowColumns::from_marktlokation(&malo);
        assert_eq!(cols.netzebene, Some("MSP_NSP_UMSP"));
        assert_eq!(cols.gasqualitaet, Some("L_GAS"));
        assert_eq!(cols.energierichtung, Some("EINSP"));
        assert_eq!(cols.bilanzierungsmethode, Some("RLM"));
    }

    /// Every emitted value round-trips through the strict BO4E parser, so a
    /// stored column can always be read back as the enum it came from.
    #[test]
    fn every_variant_round_trips_through_from_wire() {
        for &v in Netzebene::VARIANTS {
            let malo = Marktlokation {
                netzebene: Some(v),
                ..Default::default()
            };
            let wire = MaloShadowColumns::from_marktlokation(&malo)
                .netzebene
                .expect("set");
            assert_eq!(Netzebene::from_wire(wire), Ok(v));
        }
        for &v in Energierichtung::VARIANTS {
            let malo = Marktlokation {
                energierichtung: Some(v),
                ..Default::default()
            };
            let wire = MaloShadowColumns::from_marktlokation(&malo)
                .energierichtung
                .expect("set");
            assert_eq!(Energierichtung::from_wire(wire), Ok(v));
        }
    }

    /// An empty BO leaves every column `NULL` rather than inventing a default.
    #[test]
    fn an_empty_marktlokation_yields_no_columns() {
        assert_eq!(
            MaloShadowColumns::from_marktlokation(&Marktlokation::default()),
            MaloShadowColumns::default()
        );
    }
}

/// The `malo` table's enum `CHECK` lists, pinned to the BO4E schema.
///
/// A `CHECK` list is a hand-copied duplicate of an upstream enum. A schema bump
/// that adds a variant would otherwise make it reject valid data — surfacing as
/// a 500 on one tenant's `PUT`, months later. Comparing the migration against
/// `VARIANTS` makes that a build failure naming the column and the value.
#[cfg(test)]
mod check_constraint_drift {
    use std::collections::BTreeSet;

    /// Parse `col IN ('A', 'B', …)` out of a table's DDL.
    fn check_list(ddl: &str, column: &str) -> BTreeSet<String> {
        let needle = format!("CHECK ({column} IN (");
        let start = ddl
            .find(&needle)
            .unwrap_or_else(|| panic!("no CHECK constraint on `{column}` in the malo DDL"))
            + needle.len();
        let end = start
            + ddl[start..]
                .find("))")
                .expect("unterminated CHECK constraint");
        ddl[start..end]
            .split(',')
            .map(|v| v.trim().trim_matches('\'').to_owned())
            .filter(|v| !v.is_empty())
            .collect()
    }

    /// The DDL of one `CREATE TABLE`, by name.
    fn table_ddl<'a>(sql: &'a str, table: &str) -> &'a str {
        let head = format!("CREATE TABLE {table} (");
        let start = sql
            .find(&head)
            .unwrap_or_else(|| panic!("no `{table}` table in the migration"));
        let end = start
            + sql[start..]
                .find("\n);")
                .unwrap_or_else(|| panic!("unterminated `{table}` table"));
        &sql[start..end]
    }

    #[test]
    fn bo4e_check_constraints_match_the_schema() {
        let sql = include_str!("../../../../services/marktd/migrations/0001_initial.sql");

        for (table, lists) in [
            ("malo", super::malo_enum_check_lists()),
            ("melo", super::melo_enum_check_lists()),
            ("nelo", super::nelo_enum_check_lists()),
            ("partners", super::partner_enum_check_lists()),
        ] {
            let ddl = table_ddl(sql, table);
            for (column, variants) in lists {
                let expected: BTreeSet<String> = variants.iter().map(|v| (*v).to_owned()).collect();
                let actual = check_list(ddl, column);
                assert_eq!(
                    actual,
                    expected,
                    "{table}.{column}: the CHECK list has drifted from the BO4E schema. \
                     Missing: {:?}. Unknown to BO4E: {:?}.",
                    expected.difference(&actual).collect::<Vec<_>>(),
                    actual.difference(&expected).collect::<Vec<_>>(),
                );
            }
        }
    }
}

#[cfg(test)]
mod preistyp_tests {
    use super::{MAKO_PREISTYP_ATTRIBUT, is_bo4e_preistyp, position_preistyp};

    #[test]
    fn a_bo4e_preistyp_is_read_from_the_bo4e_field() {
        let pos = serde_json::json!({ "preistyp": "GRUNDPREIS" });
        assert_eq!(position_preistyp(&pos), "GRUNDPREIS");
        assert!(is_bo4e_preistyp("GRUNDPREIS"));
    }

    #[test]
    fn a_mako_preistyp_is_read_from_the_zusatz_attribut() {
        let pos = serde_json::json!({
            "zusatzAttribute": [{ "name": MAKO_PREISTYP_ATTRIBUT, "wert": "EEG_MARKTPRAEMIE" }]
        });
        assert_eq!(position_preistyp(&pos), "EEG_MARKTPRAEMIE");
        // …and it is deliberately not a BO4E value, which is the whole point.
        assert!(!is_bo4e_preistyp("EEG_MARKTPRAEMIE"));
    }

    /// `UNKNOWN` is the catch-all's own wire spelling, not a price type.
    #[test]
    fn unknown_is_not_a_bo4e_preistyp() {
        assert!(!is_bo4e_preistyp("UNKNOWN"));
        assert!(!is_bo4e_preistyp(""));
    }

    #[test]
    fn a_position_with_neither_reads_empty() {
        assert_eq!(position_preistyp(&serde_json::json!({})), "");
        assert_eq!(
            position_preistyp(&serde_json::json!({ "zusatzAttribute": [] })),
            ""
        );
    }
}
