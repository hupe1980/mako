//! Guards `vertragd`'s SQL `CHECK` lists against the Rust enums that write them.
//!
//! A `CHECK` list is opaque text to the compiler, so nothing otherwise ties it
//! to the enum on the other side of the column. Drift is silent in both
//! directions and the two directions fail differently:
//!
//! - A variant the `CHECK` does not list is a **write** that Postgres refuses.
//!   Loud, but at the customer rather than in CI.
//! - A value the `CHECK` allows and the enum does not know is a **read** that
//!   decodes to whatever the fallback says. `Vertragsart::from_db` answers
//!   `Sondervertrag` for anything it does not recognise — deliberately, so a
//!   typo cannot grant Grundversorgungs-Fristen — which means a drifted column
//!   value silently reclassifies a contract's statutory regime instead of
//!   failing. `Kuendigungsgrund::from_db` has the same shape.
//!
//! So the enum is the authority and the list is proved against it, both ways:
//! every variant appears, and every listed value decodes back to the variant
//! that wrote it.

use std::path::PathBuf;

use mako_markt::utilmd_stammdaten::{NetznutzungZahler, Netznutzungsvertrag};
use mako_service::schema_check::{check_values, check_values_in};
use vertragd::bestellung::{DokumentArt, Kanal};
use vertragd::domain::{Kuendigungsgrund, Lieferbeginnanlass, Vertragsart};
use vertragd::handlers::bestellungen::WiderrufKanal;

fn migration_sql() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_schema.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Hold one enum against one column, in both directions.
fn assert_pinned<T: Copy + PartialEq + std::fmt::Debug>(
    column: &str,
    all: &[T],
    as_db: fn(T) -> &'static str,
    from_db: fn(&str) -> Option<T>,
) {
    assert_listed(
        column,
        &check_values(&migration_sql(), column),
        all,
        as_db,
        from_db,
    );
}

/// [`assert_pinned`] for a column whose name repeats across tables (`kanal`),
/// read from one table's `CREATE TABLE` block.
fn assert_pinned_in<T: Copy + PartialEq + std::fmt::Debug>(
    table: &str,
    column: &str,
    all: &[T],
    as_db: fn(T) -> &'static str,
    from_db: fn(&str) -> Option<T>,
) {
    let listed = check_values_in(&migration_sql(), table, column);
    assert_listed(&format!("{table}.{column}"), &listed, all, as_db, from_db);
}

fn assert_listed<T: Copy + PartialEq + std::fmt::Debug>(
    column: &str,
    listed: &[String],
    all: &[T],
    as_db: fn(T) -> &'static str,
    from_db: fn(&str) -> Option<T>,
) {
    for v in all {
        assert!(
            listed.iter().any(|l| l == as_db(*v)),
            "`{column}` CHECK does not list {:?} (`{}`) — writing it would be refused by \
             Postgres. Listed: {listed:?}",
            v,
            as_db(*v)
        );
    }

    for value in listed {
        let decoded = from_db(value).unwrap_or_else(|| {
            panic!(
                "`{column}` CHECK allows `{value}`, which no variant of this enum writes — a \
                 row holding it decodes to the fallback rather than failing"
            )
        });
        assert_eq!(
            as_db(decoded),
            value.as_str(),
            "`{column}` value `{value}` decodes to {decoded:?}, which writes back as \
             `{}` — the round trip does not close",
            as_db(decoded)
        );
    }

    assert_eq!(
        listed.len(),
        all.len(),
        "`{column}` CHECK lists {} value(s) against {} enum variant(s): {listed:?}",
        listed.len(),
        all.len()
    );
}

/// `Vertragsart` decides every statutory deadline, and its fallback hides drift.
#[test]
fn vertragsart_matches_its_check_list() {
    assert_pinned(
        "vertragsart",
        Vertragsart::ALL,
        Vertragsart::as_db,
        // `from_db` answers `Sondervertrag` for anything, so it cannot report an
        // unknown. Round-tripping through `as_db` is what detects one.
        |s| {
            let v = Vertragsart::from_db(s);
            (v.as_db() == s).then_some(v)
        },
    );
}

/// The Kündigungsgrund, not the contract, decides the notice period.
#[test]
fn kuendigungsgrund_matches_its_check_list() {
    assert_pinned(
        "kuendigung_grund",
        Kuendigungsgrund::ALL,
        Kuendigungsgrund::as_db,
        |s| {
            let v = Kuendigungsgrund::from_db(s);
            (v.as_db() == s).then_some(v)
        },
    );
}

/// The Anlass is the GeLi Gas Anmeldung's Transaktionsgrund; an unlisted one
/// cannot be stored, an unknown stored one would be no Anmeldung at all.
#[test]
fn lieferbeginnanlass_matches_its_check_list() {
    assert_pinned(
        "lieferbeginn_anlass",
        Lieferbeginnanlass::ALL,
        Lieferbeginnanlass::as_db,
        Lieferbeginnanlass::from_db,
    );
}

/// The Netznutzung pair is `mako_markt`'s wire vocabulary, which the 44001 body
/// carries as it is stored.
#[test]
fn netznutzung_matches_its_check_lists() {
    const VERTRAEGE: &[Netznutzungsvertrag] = &[
        Netznutzungsvertrag::KundeNb,
        Netznutzungsvertrag::LieferantNb,
    ];
    const ZAHLER: &[NetznutzungZahler] = &[NetznutzungZahler::Kunde, NetznutzungZahler::Lieferant];
    assert_pinned(
        "netznutzungsvertrag",
        VERTRAEGE,
        Netznutzungsvertrag::as_str,
        Netznutzungsvertrag::from_wire,
    );
    assert_pinned(
        "netznutzung_zahler",
        ZAHLER,
        NetznutzungZahler::as_str,
        NetznutzungZahler::from_wire,
    );
    assert_eq!(Netznutzungsvertrag::VARIANTS.len(), VERTRAEGE.len());
    assert_eq!(NetznutzungZahler::VARIANTS.len(), ZAHLER.len());
}

/// The three lists are the ones this file believes it is reading.
///
/// `mako_service::schema_check` refuses an empty parse, so this is not about
/// that — it is about the anchors resolving to *these* columns.
#[test]
fn the_guard_reads_the_lists_it_names() {
    let sql = migration_sql();
    assert_eq!(check_values(&sql, "kuendigung_grund").len(), 4);
    assert_eq!(check_values(&sql, "vertragsart").len(), 3);
}

/// The Grundlage and the Grund der Privilegierung nach EnFG are `mako_markt`'s
/// code lists, which the 55001 body carries as they are stored (UTILMD AHB
/// Strom 2.2 Kap. 8.2, S. 85).
#[test]
fn enfg_matches_its_check_lists() {
    for (column, codes) in [
        (
            "enfg_grundlage",
            mako_markt::commands::ENFG_GRUNDLAGEN
                .iter()
                .map(|(c, _)| *c)
                .collect::<Vec<_>>(),
        ),
        (
            "enfg_grund",
            mako_markt::commands::ENFG_PRIVILEGIERUNGEN
                .iter()
                .map(|(c, _)| *c)
                .collect(),
        ),
    ] {
        let listed = check_values(&migration_sql(), column);
        assert_eq!(listed, codes, "`{column}` CHECK and its code list disagree");
    }
}

/// An order cites its documents by art; an art `rechtsdokumente` cannot store
/// is a document no hash can resolve to. `PREISBLATT` lives in `productd` and
/// is deliberately not listed.
#[test]
fn rechtsdokument_art_matches_its_check_list() {
    assert_pinned_in(
        "rechtsdokumente",
        "art",
        &DokumentArt::RECHTSDOKUMENTE,
        DokumentArt::as_db,
        DokumentArt::from_db,
    );
}

/// The channel an order came by is part of its Bestellnachweis.
#[test]
fn bestellkanal_matches_its_check_list() {
    assert_pinned_in(
        "bestellnachweise",
        "kanal",
        &Kanal::ALL,
        Kanal::as_db,
        Kanal::from_db,
    );
}

/// The channel a Widerruf came by is part of the § 356a Abs. 2 BGB record.
#[test]
fn widerrufkanal_matches_its_check_list() {
    assert_pinned_in(
        "widerrufe",
        "kanal",
        &WiderrufKanal::ALL,
        WiderrufKanal::as_db,
        WiderrufKanal::from_db,
    );
}

/// The Kündigung beim Vorversorger's state decides whether the Anmeldung is
/// held, so an unlisted state cannot be stored and an unknown one is no state.
#[test]
fn kuendigung_status_matches_its_check_list() {
    use vertragd::kuendigung_vorversorger::KuendigungStatus;
    assert_pinned(
        "kuendigung_status",
        KuendigungStatus::ALL,
        KuendigungStatus::as_db,
        KuendigungStatus::from_db,
    );
}
