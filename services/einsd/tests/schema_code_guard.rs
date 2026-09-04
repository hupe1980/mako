//! Text-level guards tying `einsd`'s SQL to its schema.
//!
//! The database tests in `settlement_integration.rs` prove what the schema
//! permits. These prove the service actually issues that form — a distinction
//! that matters, because both bugs they guard against were in the service's
//! query text while the schema was correct all along. They need no database and
//! so run on every `cargo test`.

const PG: &str = include_str!("../src/pg.rs");
const HANDLERS: &str = include_str!("../src/handlers.rs");
const MCP: &str = include_str!("../src/mcp_server.rs");
const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Strip comments so a rule cannot be satisfied — or broken — by prose.
///
/// Both `--` (SQL) and `//` (Rust, including doc comments): a guard that fires on
/// a sentence describing the old behaviour is a guard nobody trusts.
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| {
            let l = match l.find("--") {
                Some(i) => &l[..i],
                None => l,
            };
            match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `ON CONFLICT` on `settlement_receipts` must repeat the index predicate.
///
/// `sr_unique_initial` is partial (`WHERE is_correction = false`). Postgres
/// cannot infer a partial index from the column list, so the bare form raises
/// "no unique or exclusion constraint matching the ON CONFLICT specification" at
/// runtime. The award-expired settlement path shipped with the bare form and
/// failed on every call.
#[test]
fn receipt_upserts_repeat_the_partial_index_predicate() {
    let code = code_only(PG);
    // Anchored on the target table: other tables key on the same
    // (tr_id, tenant, billing_year, billing_month) tuple with a total
    // constraint, and their upserts must not be held to this predicate.
    let inserts: Vec<&str> = code
        .match_indices("INSERT INTO settlement_receipts")
        .map(|(i, _)| {
            let rest = &code[i..];
            &rest[..rest.find("RETURNING").unwrap_or(rest.len().min(1600))]
        })
        .collect();

    assert!(
        !inserts.is_empty(),
        "expected receipt upserts to exist in pg.rs"
    );
    for c in &inserts {
        if !c.contains("ON CONFLICT") {
            continue;
        }
        assert!(
            c.contains("is_correction = false"),
            "an ON CONFLICT on settlement_receipts omits the partial-index \
             predicate and will fail at runtime:\n{c}"
        );
    }
}

/// The schema must actually define that index as partial.
///
/// If it were ever made total, the predicate above would become wrong rather
/// than merely redundant — so the two are asserted together.
#[test]
fn the_receipts_unique_index_is_partial() {
    let schema = code_only(SCHEMA);
    let idx = schema
        .find("CREATE UNIQUE INDEX sr_unique_initial")
        .expect("sr_unique_initial must exist");
    let stmt = &schema[idx..schema[idx..].find(';').map_or(schema.len(), |e| idx + e)];
    assert!(
        stmt.contains("WHERE is_correction = false"),
        "sr_unique_initial must stay partial: {stmt}"
    );
}

/// Extract the SQL raw-string literals that touch `eeg_anlagen`.
///
/// Scoped to SQL because the same name may legitimately be a Rust field: the
/// settlement input really does carry a `kwk_max_kwh` value — it is simply
/// computed rather than selected.
fn eeg_anlagen_queries(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("r\"") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('"') else { break };
        let lit = &after[..end];
        if lit.contains("eeg_anlagen") {
            out.push(lit);
        }
        rest = &after[end + 1..];
    }
    out
}

/// The columns `eeg_anlagen` actually defines.
fn eeg_anlagen_columns() -> std::collections::BTreeSet<String> {
    let schema = code_only(SCHEMA);
    let start = schema
        .find("CREATE TABLE eeg_anlagen")
        .expect("eeg_anlagen must exist");
    let body = &schema[start..start + schema[start..].find("\n);").expect("table ends")];
    body.lines()
        .skip(1)
        .filter_map(|l| {
            let t = l.trim();
            let first = t.split_whitespace().next()?;
            // Column definitions start with a bare identifier followed by a type;
            // constraints and continuation lines do not.
            let is_ident = !first.is_empty()
                && first
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            let looks_like_a_type = t
                .split_whitespace()
                .nth(1)
                .is_some_and(|ty| ty.chars().next().is_some_and(char::is_uppercase));
            (is_ident && looks_like_a_type).then(|| first.to_owned())
        })
        .collect()
}

/// No `INSERT INTO eeg_anlagen` may name a column the schema does not define.
///
/// The column list is read out of the real DDL, not hardcoded: a column added to
/// the Rust binding list and forgotten in the DDL takes every registration down
/// with a 422, which is the class of bug this guard exists for.
#[test]
fn inserts_only_name_columns_that_exist() {
    let columns = eeg_anlagen_columns();
    assert!(
        columns.contains("tr_id") && columns.contains("settlement_model"),
        "the DDL parser lost track of the column list: {columns:?}"
    );

    for (name, src) in [
        ("pg.rs", PG),
        ("mcp_server.rs", MCP),
        ("handlers.rs", HANDLERS),
    ] {
        for q in eeg_anlagen_queries(src) {
            let Some(at) = q.find("INSERT INTO eeg_anlagen") else {
                continue;
            };
            let rest = &q[at..];
            let open = rest.find('(').expect("column list");
            let close = rest
                .find(") VALUES")
                .or_else(|| rest.find("\n           ) VALUES"));
            let close = close.unwrap_or_else(|| rest.find(')').expect("column list ends"));
            for col in rest[open + 1..close].split(',') {
                let col = col.trim();
                if col.is_empty() {
                    continue;
                }
                assert!(
                    columns.contains(col),
                    "{name} inserts into eeg_anlagen.{col}, which the schema does not define"
                );
            }
        }
    }
}

/// No query may name a value that is computed rather than stored.
///
/// `get_compliance_status` selected `kwk_max_kwh`, which is derived
/// (`kwk_foerderdauer_h × leistung_kwp`) and has never been a column, so the
/// tool failed for every plant.
#[test]
fn queries_do_not_name_derived_values_as_columns() {
    let columns = eeg_anlagen_columns();
    for derived in ["kwk_max_kwh"] {
        assert!(
            !columns.contains(derived),
            "{derived} is derived, not stored — this guard assumes that"
        );
        for (name, src) in [("pg.rs", PG), ("mcp_server.rs", MCP)] {
            for q in eeg_anlagen_queries(src) {
                assert!(
                    !q.contains(derived),
                    "a query in {name} names the derived value {derived} as if \
                     it were a column:\n{q}"
                );
            }
        }
    }
}

/// Every column the correction path writes must exist.
///
/// `correction_reason` was accepted from the caller, echoed back in the
/// response, and never stored — so the § 147 AO / GoBD audit trail lost the stated
/// reason for every correction.
#[test]
fn the_correction_audit_columns_are_written() {
    let schema = code_only(SCHEMA);
    let code = code_only(PG);
    for col in ["correction_of", "correction_reason", "is_correction"] {
        assert!(schema.contains(col), "{col} must exist in the schema");
        assert!(
            code.contains(col),
            "{col} exists in the schema but pg.rs never writes it"
        );
    }
}

/// A state change must be recorded, not only applied.
///
/// `settlement_state` was updated in place, so the prior value was
/// unrecoverable and the history tool always returned empty.
#[test]
fn settlement_state_changes_are_recorded_as_transitions() {
    let code = code_only(PG);
    assert!(
        code.contains("INSERT INTO settlement_state_transitions"),
        "pg.rs updates settlement_state but never records the transition"
    );
    assert!(
        code.contains("FOR UPDATE"),
        "the prior state must be read under a row lock so the recorded \
         from_state cannot race another settlement"
    );
}

/// Extract the single-quoted values of a `CHECK (<col> IN ( … ))` list.
fn check_in_values(schema: &str, col: &str) -> Vec<String> {
    let needle = format!("CHECK ({col} IN (");
    let start = schema
        .find(&needle)
        .unwrap_or_else(|| panic!("no CHECK (…) IN list for column {col}"))
        + needle.len();
    let end = start
        + schema[start..]
            .find("))")
            .expect("CHECK IN list must close with `))`");
    let list = &schema[start..end];
    let mut out = Vec::new();
    let mut rest = list;
    while let Some(q1) = rest.find('\'') {
        let after = &rest[q1 + 1..];
        let Some(q2) = after.find('\'') else { break };
        out.push(after[..q2].to_owned());
        rest = &after[q2 + 1..];
    }
    out
}

/// `eeg_anlagen.erzeugungsart` CHECK must equal `ErzeugungsArt`'s canonical
/// `to_db_str` vocabulary — in **both** directions.
///
/// The two drifted: the enum emitted `BIOMETHAN` / `SOLAR_FREIFLAECHE` while the
/// CHECK allowed `BIOMETHANE` / `SOLAR_FREFLAECHE` and an orphan `SONSTIGE` with
/// no variant. Because `pg.rs` binds the raw caller string and reads back via
/// `from_db_str(...).ok()`, a biomethane plant was either rejected by the CHECK
/// or silently decoded to `erzeugungsart = None`. This pins the two lists equal
/// so any future rename fails a DB-free test instead of losing data at runtime.
#[test]
fn erzeugungsart_check_list_equals_the_enum_vocabulary() {
    use eeg_billing::ErzeugungsArt;
    use std::collections::BTreeSet;

    let schema = code_only(SCHEMA);
    let check: BTreeSet<String> = check_in_values(&schema, "erzeugungsart")
        .into_iter()
        .collect();
    let enum_vocab: BTreeSet<String> = ErzeugungsArt::ALL
        .iter()
        .map(|a| a.to_db_str().to_owned())
        .collect();

    assert_eq!(
        check,
        enum_vocab,
        "erzeugungsart CHECK list and ErzeugungsArt::to_db_str have diverged.\n\
         in CHECK but not enum: {:?}\n\
         in enum but not CHECK: {:?}",
        check.difference(&enum_vocab).collect::<Vec<_>>(),
        enum_vocab.difference(&check).collect::<Vec<_>>(),
    );

    // Every allowed value must round-trip through the enum.
    for v in &check {
        let parsed = ErzeugungsArt::from_db_str(v)
            .unwrap_or_else(|_| panic!("CHECK value {v:?} does not parse via from_db_str"));
        assert_eq!(
            parsed.to_db_str(),
            v,
            "from_db_str/to_db_str do not round-trip for {v:?}"
        );
    }
}

/// `eeg_gesetz` CHECK must equal the `EegGesetz` year set.
///
/// A settlement stored under a year the enum cannot decode would settle under
/// the wrong EEG regime; this pins the SQL list to the Rust source of truth.
#[test]
fn eeg_gesetz_check_list_equals_the_enum_years() {
    use eeg_billing::EegGesetz;
    use std::collections::BTreeSet;

    let schema = code_only(SCHEMA);
    let needle = "CHECK (eeg_gesetz IN (";
    let start = schema.find(needle).expect("eeg_gesetz CHECK must exist") + needle.len();
    let end = start + schema[start..].find(')').expect("closes");
    let check: BTreeSet<i16> = schema[start..end]
        .split(',')
        .filter_map(|t| t.trim().parse::<i16>().ok())
        .collect();

    // Every CHECK year must decode to a concrete regime (0 = KWKG sentinel).
    for &y in &check {
        assert!(
            EegGesetz::from_db_year(y).is_ok(),
            "eeg_gesetz CHECK allows {y}, which EegGesetz::from_db_year rejects"
        );
    }
}

// ── The settlement-model vocabulary ──────────────────────────────────────────

/// Pull the quoted tokens out of the `CHECK (… IN (…))` that follows `anchor`.
fn check_tokens_after(anchor: &str) -> Vec<String> {
    let at = SCHEMA
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor `{anchor}` not found in the schema"));
    let rest = &SCHEMA[at..];
    let open = rest.find("IN (").expect("CHECK … IN (…)") + 4;
    let mut depth = 1usize;
    let mut close = open;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut chars = rest[open..close].chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }
        let mut tok = String::new();
        for c in chars.by_ref() {
            if c == '\'' {
                break;
            }
            tok.push(c);
        }
        out.push(tok);
    }
    out
}

/// `models::ALL` and the schema's `CHECK` list must name exactly the same tokens.
///
/// The two drifted before: the schema accepted a German and an English spelling
/// of every model, the service listed both in most gates and one in others, and
/// the KWKG kWh counter silently stopped applying to any plant registered under
/// the spelling it had missed. Asserting the two lists equal is what makes a
/// third spelling impossible to add halfway.
#[test]
fn the_settlement_model_vocabulary_is_single_sourced() {
    let mut from_schema = check_tokens_after("settlement_model   TEXT");
    let mut from_code: Vec<String> = einsd::models::ALL.iter().map(|s| (*s).to_owned()).collect();
    from_schema.sort();
    from_code.sort();
    assert_eq!(
        from_schema, from_code,
        "eeg_anlagen.settlement_model CHECK and einsd::models::ALL disagree"
    );
}

/// Every model the schema accepts must have a settlement branch.
///
/// A token the schema permits and `run_settlement` does not know bails the whole
/// settlement at runtime — for a plant that registered successfully.
#[test]
fn every_accepted_model_has_a_settlement_branch() {
    let code = code_only(PG);
    for token in einsd::models::ALL {
        assert!(
            code.contains(&format!("\"{token}\"")),
            "settlement_model {token} is accepted by the schema but has no branch in pg.rs"
        );
    }
}

/// The plant record's `verguetungsform` values and the tariff table's must match.
///
/// The lookup joins one against the other; a value that exists on a plant and not
/// in the rate table silently returns no rate.
#[test]
fn verguetungsform_vocabularies_match() {
    let mut plant = check_tokens_after("verguetungsform    TEXT");
    let mut table = check_tokens_after("verguetungsform     TEXT");
    plant.sort();
    table.sort();
    assert_eq!(
        plant, table,
        "eeg_anlagen.verguetungsform and eeg_verguetungssaetze.verguetungsform disagree"
    );
}

/// The tariff-rate lookup must filter on `verguetungsform`.
///
/// Überschuss and Volleinspeisung rates share a band and a start date and differ
/// by the §48 Abs. 2a bonus. Without the filter, which of the two a plant is
/// seeded with came down to row order.
#[test]
fn the_tariff_lookup_filters_on_verguetungsform() {
    let code = code_only(PG);
    let at = code
        .find("FROM eeg_verguetungssaetze")
        .expect("the rate lookup must exist");
    let stmt = &code[at..at + 500.min(code.len() - at)];
    assert!(
        stmt.contains("verguetungsform"),
        "the rate lookup ignores verguetungsform:\n{stmt}"
    );
}

/// The tariff-table seed must not swallow its own key collisions.
///
/// With `ON CONFLICT DO NOTHING` the entire §48 Abs. 2a Volleinspeisung block
/// collides with the Überschuss rows on
/// `(erzeugungsart, leistung_min_kwp, billing_start)`, and every one of those
/// rates is dropped by a migration that reports success.
#[test]
fn the_tariff_seed_does_not_swallow_collisions() {
    let seed_at = SCHEMA
        .find("INSERT INTO eeg_verguetungssaetze")
        .expect("the rate seed must exist");
    let seed = &SCHEMA[seed_at..];
    let end = seed.find(";\n").map_or(seed.len(), |e| e + 1);
    assert!(
        !seed[..end].contains("ON CONFLICT"),
        "the eeg_verguetungssaetze seed must fail loudly on a key collision"
    );
}

/// The §51 auto-derivation must take the billing month in German local time.
///
/// EPEX day-ahead prices and edmd's ¼h series are both published for the German
/// market time. A month taken from midnight UTC is an hour out of phase — two at
/// DST — so its first hour matches the previous month's prices and its last hour
/// falls outside the window entirely.
#[test]
fn the_billing_month_window_is_german_local_time() {
    let code = code_only(HANDLERS);
    let at = code
        .find("fn billing_month_range")
        .expect("billing_month_range must exist");
    let body = &code[at..at + 1400.min(code.len() - at)];
    assert!(
        body.contains("mako_fristen::berlin_midnight"),
        "the billing-month window must be tiled from Berlin midnights"
    );
    assert!(
        !body.contains("assume_utc"),
        "the billing-month window must not be taken in UTC"
    );
}

// ── One settle path ──────────────────────────────────────────────────────────

const SETTLE: &str = include_str!("../src/settle.rs");
const MAIN: &str = include_str!("../src/main.rs");
const SECT52: &str = include_str!("../src/sect52.rs");

/// Only `settle.rs` may run a settlement.
///
/// REST, batch, MCP `trigger_settle` and the monthly worker are four entry points
/// to one payment obligation. Each assembling its own `SettleOverrides` lets them
/// drift, so the same plant is paid differently depending on which one ran.
#[test]
fn only_one_module_calls_run_settlement() {
    for (name, src) in [
        ("handlers.rs", HANDLERS),
        ("mcp_server.rs", MCP),
        ("main.rs", MAIN),
    ] {
        let code = code_only(src);
        assert!(
            !code.contains("run_settlement("),
            "{name} calls run_settlement directly — settlements go through \
             settle::settle_plant so the amount cannot depend on the entry point"
        );
    }
    assert!(
        code_only(SETTLE).contains("run_settlement("),
        "settle.rs must be the one that runs it"
    );
}

/// Every entry point resolves the §51 figures, because they all share one path.
#[test]
fn the_shared_settle_path_derives_the_sect51_figures() {
    let code = code_only(SETTLE);
    assert!(
        code.contains("derive_negativpreis_from_edmd"),
        "the shared settle path must derive §51/§51a when the caller supplies neither"
    );
    assert!(
        code.contains("fetch_einspeisemenge_from_edmd"),
        "the shared settle path must fetch the Einspeisemenge when not supplied"
    );
}

/// §52 detection lives in one module.
///
/// Detecting them inline in `run_settlement` lets a rule go half-present: indexed
/// but never queried, or reported by an MCP tool but never reaching a settlement.
#[test]
fn sect52_violations_are_derived_in_one_place() {
    for (name, src) in [
        ("pg.rs", PG),
        ("mcp_server.rs", MCP),
        ("handlers.rs", HANDLERS),
    ] {
        let code = code_only(src);
        // Naming a `SanktionsTyp` is fine — the register stores one and the REST
        // surface parses one. *Building a `Pflichtverstoss`* is what has to stay
        // in one place, so the §52 Abs. 3 flags and the Abs. 4 month extension
        // cannot be applied on one path and forgotten on another.
        assert!(
            !code.contains("Pflichtverstoss {"),
            "{name} constructs a §52 violation — that belongs in sect52.rs"
        );
    }
    let sect52 = code_only(SECT52);
    for typ in [
        "FernsteuerbarkeitFehlend",
        "Sect10bVorgabenVerletzt",
        "AusfallverguetungHoechstdauerUeberschritten",
        "ZuordnungsWechselNichtGemeldet",
        "MastrNichtRegistriert",
    ] {
        assert!(sect52.contains(typ), "sect52.rs no longer derives {typ}");
    }
}

/// The `eeg_pflichtverstoesse.typ` CHECK is the `SanktionsTyp` vocabulary.
///
/// The register is the only path by which nine of the thirteen §52 Abs. 1
/// Nummern ever reach a settlement, so a breach the enum knows and the CHECK
/// rejects is a breach that cannot be filed at all — and one the CHECK accepts
/// and the enum does not is a row `list_pflichtverstoesse` silently skips.
#[test]
fn the_pflichtverstoss_check_matches_the_enum() {
    let schema = code_only(SCHEMA);
    let start = schema
        .find("CREATE TABLE eeg_pflichtverstoesse")
        .expect("the register table exists");
    let table = &schema[start..schema[start..].find(");").expect("table ends") + start];
    for typ in eeg_billing::SanktionsTyp::ALL {
        assert!(
            table.contains(&format!("'{}'", typ.as_db_str())),
            "eeg_pflichtverstoesse.typ rejects §52 Abs. 1 Nr. {} ({})",
            typ.nummer(),
            typ.as_db_str()
        );
    }
    let in_check = table.matches('\'').count() / 2;
    assert_eq!(
        in_check,
        eeg_billing::SanktionsTyp::ALL.len(),
        "the CHECK admits a token the enum does not know"
    );
}

/// The §9 obligation is staged by capacity, so a flat capacity test is a bug.
///
/// A flat "≥ 25 kW needs Fernsteuerbarkeit" charged 10 €/kW/month to every
/// compliant plant in the 25–100 kW band that took the 60 % Leistungsbegrenzung
/// §9 Abs. 2 Nr. 2 offers it.
#[test]
fn sect9_compliance_is_not_a_bare_capacity_test() {
    let code = code_only(SECT52);
    assert!(
        code.contains("sect9_verletzt"),
        "§9 compliance must go through the staged helper"
    );
    assert!(
        !code.contains("fernsteuerbarkeit_datum.is_none()"),
        "a bare 'no Fernsteuerbarkeit date' test ignores the 60 % Leistungsbegrenzung route"
    );
}

/// Money does not cross the MCP surface as `f64`.
///
/// 0,1 ct/kWh has no exact `f64`, and these values reach a legally binding
/// §14 UStG Gutschrift.
#[test]
fn the_mcp_surface_takes_exact_decimals() {
    let code = code_only(MCP);
    for field in [
        "leistung_kwp: f64",
        "avg_ct_kwh: f64",
        "einspeisemenge_kwh: Option<f64>",
        "epex_avg_ct_kwh: Option<f64>",
    ] {
        assert!(
            !code.contains(field),
            "the MCP surface still takes `{field}` — money and energy use DecimalArg"
        );
    }
}

// ── The MCP prompts are instructions a model acts on ─────────────────────────

/// No prompt may teach a rule the engine does not implement.
///
/// The six prompts shipped an additive Managementprämie of 0,4 ct/kWh — Anlage 1
/// Nr. 3.1.2 defines `MP = AW − MW` and nothing else, and §20 EEG 2023 has no
/// Absätze at all — a 20-year clock reset under "§22" (the Ausschreibung
/// provision), a twelve-month advance-notice duty under "§21 Abs. 1" that no such
/// provision contains, and MaStR maintenance under "§28a". A wrong instruction is
/// worse than none: the model acts on it.
#[test]
fn the_mcp_prompts_do_not_teach_refuted_rules() {
    // The prose that documents *why* these are wrong is allowed to name them;
    // the instruction text a model receives is not.
    let prompts = {
        let at = MCP
            .find("#[prompt_router]")
            .expect("the prompt router must exist");
        code_only(&MCP[at..])
    };
    for (needle, why) in [
        // The formula forms. The term itself is allowed where a prompt denies it,
        // which is the whole point of saying so.
        (
            "+ Managementpraemie",
            "Anlage 1 defines MP = AW − MW, with the marketing cost inside the AW",
        ),
        (
            "Managementpraemie: 0.4",
            "there is no additive Managementprämie to state a rate for",
        ),
        (
            "REPOWERING sect. 22",
            "§22 is the Ausschreibung provision; repowering is §3 Nr. 30 i.V.m. §25",
        ),
        (
            "sect. 22 EEG — replace components",
            "§22 does not govern repowering",
        ),
        (
            "notify Anlagenbetreiber",
            "no EEG provision imposes an advance-notice period for the Förderende",
        ),
        (
            "sect. 28a EEG",
            "MaStR maintenance is the MaStRV i.V.m. §71, not §28a",
        ),
        (
            "sect. 25 EEG 2023 sanctions",
            "§25 is Beginn/Dauer/Beendigung des Anspruchs, not a sanction",
        ),
        ("§23b", "POST_EEG_SPOT has no §23b 10-ct cap"),
    ] {
        assert!(
            !prompts.contains(needle),
            "an MCP prompt still contains {needle:?} — {why}"
        );
    }
    // And the denial is stated where an agent computing a Marktprämie will read it.
    assert!(
        prompts.contains("no additive Managementprämie"),
        "the settle-monthly prompt must say the Marktprämie has no additive component"
    );
}

/// The prompts must name the vocabulary the schema actually accepts.
#[test]
fn the_mcp_prompts_name_the_real_settlement_models() {
    let at = MCP.find("fn get_info").expect("server info must exist");
    let instructions = &MCP[at..];
    for model in einsd::models::ALL {
        assert!(
            instructions.contains(model),
            "the MCP server instructions omit the {model} settlement model"
        );
    }
    assert!(
        !instructions.contains("Settlement models (9)"),
        "the model count in the server instructions is stale"
    );
}

/// The §44b quota is measured against the statutory hours, not a flat 8 760.
///
/// §3 Nr. 6 divides by "die Summe der vollen Zeitstunden des jeweiligen
/// Kalenderjahres abzüglich der vollen Stunden vor der erstmaligen Erzeugung" —
/// 8 784 in a leap year, and shorter for a plant's first year.
#[test]
fn the_sect44b_quota_uses_the_statutory_hours() {
    for (name, src) in [("pg.rs", PG), ("mcp_server.rs", MCP)] {
        let code = code_only(src);
        assert!(
            !code.contains("8760"),
            "{name} still hardcodes 8760 hours for a Bemessungsleistung"
        );
    }
    assert!(
        code_only(PG).contains("sect44b_jahreskontingent_kwh"),
        "the settlement must take the §44b quota from the engine"
    );
}
