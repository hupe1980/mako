//! Guard: every `ZusatzAttribut` mako emits is namespaced and registered.
//!
//! ## Why a namespace
//!
//! `ZusatzAttribut` is BO4E's sanctioned escape hatch — a `{name, wert}` pair on
//! every BO and most COMs, meant (per the reference implementation) for
//! "external references for data objects that have unique IDs in different
//! systems". It is where anything the schema does not model belongs.
//!
//! BO4E mandates **no naming convention** for it, which is exactly why a
//! producer needs one. Without a prefix, `rechnungsart` is indistinguishable
//! from a field BO4E might introduce later, and from an attribute the ERP on the
//! other side of the exchange already writes. mako's convention is
//! `mako:<snake_case>`.
//!
//! Producers reach the same documents from several crates, so the convention is
//! enforced here rather than left to each of them.
//!
//! ## Why a registry
//!
//! A consumer cannot discover what mako puts in `zusatzAttribute` by reading
//! the BO4E schema — that is the point of an extension slot. The table below is
//! the discoverable list, and adding an attribute means adding a row, which is
//! a deliberate act rather than a string typed at a call site.

use std::collections::BTreeSet;
use std::path::Path;

/// Every `ZusatzAttribut` mako is allowed to emit, with what it carries.
///
/// Keep sorted; the guard prints this as the public inventory.
const REGISTRY: &[(&str, &str)] = &[
    // ── energy-billing: the end-customer invoice ─────────────────────────────
    (
        "mako:billing_run_id",
        "the billing run that produced this invoice",
    ),
    (
        "mako:einheit",
        "the original unit where BO4E `Mengeneinheit` cannot express it",
    ),
    (
        "mako:externe_kunden_id",
        "the customer's ID in the operator's ERP",
    ),
    (
        "mako:gasqualitaet",
        "H_GAS/L_GAS, for the §147 AO / GoBD audit trail",
    ),
    ("mako:guthabenerstattung", "credit-balance refund marker"),
    (
        "mako:kilowattstundenpreis_gesamt",
        "§40a EnWG all-in ct/kWh",
    ),
    (
        "mako:kundenkategorie",
        "the operator's own customer classification",
    ),
    ("mako:marktpartnercode", "the issuing Marktpartner's code"),
    (
        "mako:preisvergleichsdaten",
        "§41 EnWG price-comparison block",
    ),
    (
        "mako:rechnungsart",
        "process label BO4E `Rechnungstyp` cannot express (Gutschrift, Storno, Korrektur, Teilrechnung)",
    ),
    ("mako:stromkennzeichnung", "§42 EnWG fuel-mix disclosure"),
    (
        "mako:verbrauch_bundesdurchschnitt",
        "§40 EnWG national-average comparison",
    ),
    ("mako:verbrauch_vorjahr", "§40 EnWG prior-year comparison"),
    (
        "mako:verbraucherinformationen",
        "§41 EnWG consumer-information block",
    ),
    (
        "mako:vertragsart",
        "contract kind (Ersatzversorgung, Sondervertrag, …)",
    ),
    ("mako:vertragsdauer", "§40 Abs. 1 EnWG contract term"),
    ("mako:kuendigungsfrist", "§40 Abs. 1 EnWG notice period"),
    (
        "mako:naechstmoeglicher_kuendigungstermin",
        "§40 Abs. 1 EnWG earliest termination date",
    ),
    (
        "mako:naechster_abrechnungstermin",
        "§40 Abs. 1 EnWG next billing date",
    ),
    // ── grid-billing: the network-use settlement ─────────────────────────────
    (
        "mako:calculation_trace",
        "why each position is the amount it is — the only record of the engine's working",
    ),
    (
        "mako:legal_references",
        "the paragraphs applied to this position",
    ),
    (
        "mako:settlement_warnings",
        "engine findings that did not block the settlement",
    ),
    ("mako:steuer_rechtsgrundlage", "the VAT provision applied"),
    (
        "mako:umsatzsteuer_hinweis",
        "the §13b / §19 note printed on the invoice",
    ),
    // ── tarifbd: the price sheet ─────────────────────────────────────────────
    (
        "mako:preistyp",
        "a price type BO4E `Preistyp` does not model (EEG, HEMS, E-Mobility, …)",
    ),
    // ── billingd: aggregate invoices whose subject BO4E has no BO for ────────
    (
        "mako:dispatch_event_count",
        "VPP dispatch events settled by this Gutschrift",
    ),
    (
        "mako:dispatch_process_ids",
        "the makod processes the dispatches came from",
    ),
    (
        "mako:flexibility_kwh",
        "flexibility delivered by one dispatch",
    ),
    (
        "mako:ggv_id",
        "the §42b Gebäudestromversorgung this invoice belongs to",
    ),
    (
        "mako:malos_count",
        "MaLos aggregated into this Sammelrechnung",
    ),
    (
        "mako:rahmenvertrag_id",
        "the framework contract the Sammelrechnung settles",
    ),
    ("mako:sr_id", "the SteuerbareRessource that was dispatched"),
    ("mako:tenant_count", "tenants sharing this GGV allocation"),
    (
        "mako:total_flexibility_kwh",
        "flexibility settled across all dispatches",
    ),
    ("mako:total_kwh", "energy allocated across the GGV"),
    ("mako:tx_id", "the TechnischeRessource that was dispatched"),
    (
        "mako:vpp_id",
        "the virtual power plant this settlement belongs to",
    ),
];

/// Scan the workspace. Returns `true` when every emitted attribute conforms.
pub fn run(workspace_root: &Path) -> bool {
    let registered: BTreeSet<&str> = REGISTRY.iter().map(|(n, _)| *n).collect();
    let mut findings: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for dir in ["crates", "services"] {
        collect(
            &workspace_root.join(dir),
            &mut seen,
            &mut findings,
            &registered,
        );
    }

    // A registry row nothing emits is as much drift as an unregistered name.
    for (name, _) in REGISTRY {
        if !seen.contains(*name) {
            findings.push(format!(
                "{name} is registered but nothing emits it — remove the row or the attribute"
            ));
        }
    }

    if findings.is_empty() {
        println!(
            "check-bo4e-attributes: {} ZusatzAttribut name(s), all `mako:`-namespaced and registered",
            REGISTRY.len()
        );
        return true;
    }
    eprintln!("check-bo4e-attributes: {} problem(s):", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nEvery ZusatzAttribut mako emits must be named `mako:<snake_case>` and\n\
         listed in REGISTRY (xtask/src/check_bo4e_attributes.rs). BO4E mandates no\n\
         convention for the extension slot, so an unprefixed name can collide with\n\
         a future BO4E field or with the counterparty's own attributes."
    );
    false
}

fn collect(
    dir: &Path,
    seen: &mut BTreeSet<String>,
    findings: &mut Vec<String>,
    registered: &BTreeSet<&str>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, seen, findings, registered);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (name, line) in emitted_names(&src) {
                seen.insert(name.clone());
                if !registered.contains(name.as_str()) {
                    let hint = if name.starts_with("mako:") {
                        "not in REGISTRY"
                    } else {
                        "not namespaced — should be `mako:<snake_case>`"
                    };
                    findings.push(format!(
                        "{}:{line}  ZusatzAttribut {name:?} — {hint}",
                        path.display()
                    ));
                }
            }
        }
    }
}

/// The `ZusatzAttribut` names a source emits, as `(name, 1-based line)`.
///
/// Two shapes count, and the distinction matters more than it looks:
///
/// * `zusatz_attribut("name", …)` — the helper, including its multi-line form.
/// * `ZusatzAttribut { name: Some("name"), … }` — the struct literal.
///
/// A bare `"name":` JSON key is **not** matched: `contact_name:
/// Some("Rechnungseingang")` and an `Ansprechpartner`'s `name` are ordinary
/// fields that share the word.
///
/// Reads are also ignored: matching a name in order to *filter* on it —
/// `a["name"] == "mako:rechnungsart"` — is not emitting it.
fn emitted_names(src: &str) -> Vec<(String, usize)> {
    let bytes = src.as_bytes();
    let line_of = |off: usize| src[..off].bytes().filter(|b| *b == b'\n').count() + 1;
    let mut out = Vec::new();

    // A constant declared for the purpose is emitted wherever it is used.
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let is_const = t.starts_with("const ")
            || t.starts_with("pub const ")
            || t.starts_with("pub(crate) const ");
        if is_const && t.contains("&str = \"mako:") {
            if let Some(q) = t.find("\"mako:") {
                let after = &t[q + 1..];
                if let Some(end) = after.find('"') {
                    out.push((after[..end].to_owned(), i + 1));
                }
            }
        }
    }

    for pat in ["zusatz_attribut(", "ZusatzAttribut {"] {
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(pat) {
            let at = from + rel;
            from = at + pat.len();
            if line_is_comment(src, at) {
                continue;
            }
            // The helper's own declaration is not a call site.
            let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
            let prefix = src[line_start..at].trim_start();
            if prefix.starts_with("fn ") || prefix.ends_with("fn ") {
                continue;
            }
            // The name is the first string literal after the opener, within a
            // short window so a multi-line call is caught but the next
            // statement is not. The window is clamped to a char boundary —
            // these files are full of `─` box-drawing rules in comments.
            let mut window_end = (at + 400).min(bytes.len());
            while window_end < bytes.len() && !src.is_char_boundary(window_end) {
                window_end += 1;
            }
            let window = &src[at..window_end];
            let hay = if pat.starts_with("ZusatzAttribut") {
                match window.find("name:") {
                    Some(i) => &window[i + "name:".len()..],
                    None => continue,
                }
            } else {
                &window[pat.len()..]
            };
            // The literal must be the very next token, so a helper call whose
            // window happens to run into the following statement contributes
            // nothing.
            let head = hay.trim_start_matches(|c: char| {
                c.is_whitespace() || c == '(' || c == ':' || c == '&'
            });
            // `name: Some("…")` — step over the wrapper.
            let head = head.strip_prefix("Some(").map_or(head, str::trim_start);
            if !head.starts_with('"') {
                continue;
            }
            if let Some(end) = head[1..].find('"') {
                let name = &head[1..=end];
                if !name.is_empty() && !name.contains(' ') && !name.contains('{') {
                    out.push((name.to_owned(), line_of(at)));
                }
            }
        }
    }
    // A call whose name argument is an identifier rather than a literal —
    // `for (name, wert) in [("x", …), …] { zusatz_attribut(name, …) }` — has
    // its names in the table above it. The §40 EnWG attributes are emitted
    // that way.
    let mut from = 0usize;
    while let Some(rel) = src[from..].find("zusatz_attribut(") {
        let at = from + rel;
        from = at + "zusatz_attribut(".len();
        if line_is_comment(src, at) {
            continue;
        }
        let after = src[from..].trim_start();
        if after.starts_with('"') {
            continue; // literal — already handled
        }
        // Only the array bound to *this* identifier counts — the nearest
        // `in [` would pick up whatever array precedes the call.
        let ident: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if ident.is_empty() {
            continue;
        }
        let binder = format!("for ({ident},");
        let Some(for_at) = src[..at].rfind(&binder) else {
            continue;
        };
        let Some(rel_arr) = src[for_at..at].find(" in [") else {
            continue;
        };
        let arr_start = for_at + rel_arr + " in [".len();
        if at.saturating_sub(arr_start) > 2000 {
            continue;
        }
        for tup in src[arr_start..at].split('(').skip(1) {
            let t = tup.trim_start();
            if let Some(rest) = t.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                let name = &rest[..end];
                if !name.is_empty() && !name.contains(' ') {
                    out.push((name.to_owned(), line_of(arr_start)));
                }
            }
        }
    }

    out.sort_by_key(|(_, l)| *l);
    out.dedup();
    out
}

/// Whether the byte offset sits on a comment line.
fn line_is_comment(src: &str, at: usize) -> bool {
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    src[start..at].trim_start().starts_with("//")
}
