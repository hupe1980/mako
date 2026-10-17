//! Guard: a JSON request body refuses what it cannot store, and gates what is
//! BO4E.
//!
//! Four rules, one scan, because all are about the same moment — the instant an
//! untrusted body becomes a Rust value — and all failed the same way: silently.
//!
//! ## Rule 1 — no body reaches a handler past the unknown-key refusal
//!
//! `serde` ignores a key no field declares, so a request naming a field the API
//! does not have succeeds and the value goes nowhere. A customer posted as
//!
//! ```json
//! { "anrede": "Frau", "vorname": "Erika", "nachname": "Mustermann",
//!   "strasse": "Musterstr. 1", "plz": "10115", "ort": "Berlin" }
//! ```
//!
//! to an endpoint taking a BO4E `geschaeftspartner` is created with no name and
//! no address, and the invoice that follows names nobody.
//!
//! `mako_service::Json` refuses that at runtime: it deserialises through
//! `serde_ignored` and answers a `422` naming every undeclared key by its JSON
//! path, at any depth, whatever attributes the type carries. The rule is
//! therefore not "every body type carries `deny_unknown_fields`" — the
//! extractor already holds every type it reads — but the places the runtime
//! check does not reach:
//!
//! - **A shape `serde` buffers.** A `#[serde(flatten)]` field, an
//!   `#[serde(untagged)]` enum and an internally tagged (`tag = "…"`) enum are
//!   read out of a buffer `serde` filled itself, out of `serde_ignored`'s
//!   sight (`mako_service` `json.rs`:
//!   `flatten_and_untagged_leftovers_are_not_seen`). Such an enum, and every
//!   type reached through one or through a flattened field, carries
//!   `#[serde(deny_unknown_fields)]` (`deny_on_the_buffered_shape_refuses_the_leftover`).
//!   A struct holding a flattened field cannot — `serde` refuses the
//!   combination — so it declares the fields instead, or the buffer is listed
//!   in [`BUFFER_EXEMPT`] with its reason.
//! - **A body extracted past `mako_service::Json`.** `Lenient<T>` skips the
//!   refusal on purpose and `axum::Json` never had it; each use is listed in
//!   [`LENIENT_USES`] or [`AXUM_JSON_USES`] with its reason, and a listed use
//!   that is gone fails until its entry goes too.
//!
//! The walk follows a body inwards — every struct field, every enum variant —
//! through `services/` and the workspace crates under `crates/`, so a type is
//! in scope because something reachable from a handler signature holds it, not
//! because of how it is named or where it is declared.
//!
//! ## Rule 2 — a BO4E document in a request body is a `Bo4e<T>`
//!
//! `mako_markt::bo4e::decode` is one function call, and the defect it keeps
//! producing is that somebody does not make it. A field typed
//! `serde_json::Value` and documented as a BO4E payload is stored exactly as it
//! arrived: wrong `_typ`, out-of-schema enums, unbounded nesting, no rules.
//! `Bo4e<T>` runs the gate as `serde` deserialises, so there is no call to
//! forget.
//!
//! The rule fires on the **field name**: a name that is a BO4E type's own,
//! after dropping a `_json` suffix and a `<something>_` prefix — `vertrag`,
//! `kosten_json`, `standort_adresse`, `geschaeftspartner`. Names like `data`,
//! `payload` and `view` name no BO4E type and are not the rule's business; a
//! handler that decodes those explicitly is doing the same job by hand.
//!
//! ## Rule 4 — the opaque-name escape hatch has to be earned
//!
//! Rule 2 fires on the field *name*, so a BO4E document in a field called
//! `data` is invisible to it. That was defended as "a handler that decodes
//! those explicitly is doing the same job by hand" — a claim nothing checked,
//! and `marktd`'s `UpsertEdgeRequest.data` was the counter-example: documented
//! as a BO4E `Lokationszuordnung`, stored verbatim, and the
//! `lokationsbuendelcode` column derived from it by string lookup.
//!
//! So a `serde_json::Value` field whose **doc comment** names a BO4E type must
//! live in a file that gates — `Bo4e<T>`, or a `bo4e::decode` call. The rule is
//! per-file rather than per-field because the gate for a `data` field is a line
//! in the handler beside it, and pinning it to the field would mean parsing the
//! handler.
//!
//! It also refuses a doc naming a BO4E type that **does not exist**. Three
//! places called `marktd`'s Tranche payload a "BO4E `Tranche`"; `BoTyp` has 39
//! members and none is `TRANCHE`. The word is in the schema as
//! `Preismodell::Tranche`, an unrelated pricing model.
//!
//! ## Rule 3 — no `\uXXXX` escape in a comment
//!
//! A doc comment reading `\u00a742 EnWG` instead of `§ 42 EnWG` is what a tool
//! writing JSON-escaped text into source leaves behind, and it renders that way
//! on docs.rs. The rule is scoped to comments: inside a string literal a `\u`
//! escape is the language's own.
//!
//! ## Exemptions
//!
//! Each is a claim that has to be argued in writing, so every list carries a
//! reason per entry and every list is short.

use std::path::Path;

/// Buffers a body reaches — a struct with a `#[serde(flatten)]` field, an
/// untagged or internally tagged enum — whose contents are not held to
/// `deny_unknown_fields`, with the reason. Keyed by file and name. An entry
/// covers everything read out of that buffer, so it is a claim about the whole
/// subtree and has to say why.
const BUFFER_EXEMPT: &[(&str, &str, &str)] = &[(
    "crates/energy-billing/src/tariff.rs",
    "Product",
    "the variant structs are also read from billingd's productd mapping \
     (`services/billingd/src/clients.rs`), which builds one object carrying every \
     category's keys for whichever variant the product is, so they cannot deny the keys \
     of the others — and two of them flatten an `ElectricityProduct`. A misspelt key in a \
     `tariff` posted to billingd is therefore not refused",
)];

/// Every `Lenient<T>` extraction, by file and body type, with the reason the
/// body is read without the unknown-key refusal.
const LENIENT_USES: &[(&str, &str, &str)] = &[(
    "services/edmd/src/server/lastgang.rs",
    "SmgwTyp2Push",
    "BSI TR-03109 defines this payload, not mako — a conformant Smart-Meter-Gateway one \
     revision ahead must be read, not refused",
)];

/// Every typed `axum::Json<T>` extraction — a body read by `serde` alone, with
/// no unknown-key refusal — with the reason.
///
/// The four are BDEW **Energy API** types (MaLo-Identifikation, the
/// API-Verzeichnis). The schema is the BDEW's, and forward compatibility is its
/// property, not mako's: a counterparty on a later minor version adds fields,
/// and refusing their whole document over one would be mako failing an
/// interoperability requirement it does not get to redefine.
const AXUM_JSON_USES: &[(&str, &str, &str)] = &[
    (
        "crates/energy-api/src/server/malo_ident.rs",
        "IdentificationParameter",
        "BDEW Energy API request schema — a later minor version may add fields",
    ),
    (
        "crates/energy-api/src/server/malo_ident.rs",
        "MaloIdentResultPositive",
        "BDEW Energy API response schema — a later minor version may add fields",
    ),
    (
        "crates/energy-api/src/server/malo_ident.rs",
        "MaloIdentResultNegative",
        "BDEW Energy API response schema — a later minor version may add fields",
    ),
    (
        "crates/energy-api/src/directory/server.rs",
        "ApiRecord",
        "BDEW API-Verzeichnis entry — a later minor version may add fields",
    ),
];

/// BO4E-named fields that legitimately stay `serde_json::Value`, with the reason.
const GATE_EXEMPT: &[(&str, &str, &str)] = &[];

/// Every BO4E type name whose snake_case spelling is a field name worth
/// checking, lowercased.
///
/// Read off `rubo4e`'s own inventory rather than guessed: these are the BOs and
/// the COMs that name a whole document somebody would put in a field.
const BO4E_FIELD_NAMES: &[&str] = &[
    "adresse",
    "angebot",
    "bilanzierung",
    "energiemenge",
    "energiemix",
    "fremdkosten",
    "geraet",
    "geschaeftspartner",
    "kosten",
    "lastgang",
    "marktlokation",
    "marktteilnehmer",
    "messlokation",
    "netzlokation",
    "person",
    "preisblatt",
    "preisgarantie",
    "rechnung",
    "standorteigenschaften",
    "tarifinfo",
    "tarifpreisblatt",
    "vertrag",
    "vorauszahlung",
    "zaehler",
    "zaehlzeitdefinition",
    "zahlungsinformation",
    "zeitreihe",
];

/// Run every rule. Returns `true` when the tree is clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    for dir in ["crates", "services"] {
        collect_rs(&workspace_root.join(dir), workspace_root, &mut files);
    }

    // Every type a handler extracts, and how; Rule 2 holds the closure of all
    // of them, Rule 1 the places the runtime refusal cannot see.
    let found_extractions = extractions(&files);
    let extracted: Vec<String> = json_body_types(&files);
    let body_types = reachable_body_types(&extracted, &files);

    // Both counts are what the success line reports. A scan that found no
    // source file, or no JSON body type in any of them, checks nothing, and
    // "0 JSON body type(s) deny unknown fields" reads exactly like a pass.
    if files.is_empty() || extracted.is_empty() {
        eprintln!(
            "check-request-bodies: the scan found {} source file(s) and {} JSON body type(s) \
             under crates/ and services/ — the layout has probably changed",
            files.len(),
            extracted.len()
        );
        return false;
    }

    let missing_deny = rule_one(&found_extractions, &files);
    let mut ungated = Vec::new();
    let mut escapes = Vec::new();

    for (rel, src) in &files {
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if (t.starts_with("///") || t.starts_with("//!") || t.starts_with("//"))
                && has_unicode_escape(line)
            {
                escapes.push(format!("{rel}:{}  {}", n + 1, t.trim()));
            }
        }
    }

    for (rel, src) in &files {
        for s in structs(src) {
            // A struct that does not derive `Deserialize` is not something
            // anyone can send: a *client-side* request builder sharing a name
            // with an inbound body — `accountingd`'s `IssueDocumentRequest`,
            // which it serialises and posts to `outputd` — is no body.
            let is_body = body_types.contains(&s.name) && s.attrs.contains("Deserialize");
            // Rule 2 is about what a **caller** sends. A repository record or a
            // read row deserialises from storage, where the document is
            // whatever an older schema series wrote and refusing it would fail
            // a `GET` on a row that merely got old.
            //
            // Body types are matched by name across the tree, and two types can
            // share one: `mako_markt::PartnerRecord` is `marktd`'s **read row**
            // and `mako_engine::PartnerRecord` is `makod`'s request body. So a
            // request body is one declared in `services/` — a domain-crate
            // record reached by a name collision is a different type with a
            // different job. Rule 1 still holds it, which is the half that
            // matters for a type somebody really does post.
            let is_request = (is_body || names_a_request(&s.name)) && rel.starts_with("services/");
            if !s.attrs.contains("Deserialize") || !is_request {
                continue;
            }
            for (field, ty) in &s.fields {
                if !ty.contains("serde_json::Value") {
                    continue;
                }
                if !names_a_bo4e_type(field) {
                    continue;
                }
                if GATE_EXEMPT
                    .iter()
                    .any(|(n, f, _)| *n == s.name && f == field)
                {
                    continue;
                }
                ungated.push(format!("{rel}  {}.{field}: {ty}", s.name));
            }
        }
    }

    // Rule 4: a `serde_json::Value` whose doc names a BO4E type, in a file that
    // gates nothing. Scanned over the source rather than through `structs()`,
    // which does not carry doc comments.
    let mut undocumented_gate = Vec::new();
    for (rel, src) in &files {
        if !rel.starts_with("services/") {
            continue;
        }
        let src = strip_test_modules(src);
        let gates = src.contains("bo4e::decode") || src.contains("Bo4e<");
        for (field, doc, line_no) in bo4e_documented_value_fields(&src) {
            if names_a_bo4e_type(&field) {
                continue; // rule 2 owns this one
            }
            if disclaims_bo4e(&doc) {
                continue;
            }
            if let Some(bogus) = names_an_unknown_bo4e_type(&doc) {
                undocumented_gate.push(format!(
                    "{rel}:{line_no}  {field}: doc names `{bogus}`, which BO4E does not define"
                ));
            } else if !gates {
                undocumented_gate.push(format!(
                    "{rel}:{line_no}  {field}: documented as BO4E, and nothing in this file gates"
                ));
            }
        }
    }

    if missing_deny.is_empty()
        && ungated.is_empty()
        && escapes.is_empty()
        && undocumented_gate.is_empty()
    {
        println!(
            "check-request-bodies: {} body type(s) reached from {} extraction(s), none past the \
             unknown-key refusal unlisted, no ungated BO4E field, no escaped comment \
             ({} buffer + {} Lenient + {} axum::Json + {} gate exemption(s))",
            body_types.len(),
            found_extractions.len(),
            BUFFER_EXEMPT.len(),
            LENIENT_USES.len(),
            AXUM_JSON_USES.len(),
            GATE_EXEMPT.len()
        );
        return true;
    }

    if !missing_deny.is_empty() {
        eprintln!(
            "ERROR: {} body shape(s) the unknown-key refusal cannot see:\n",
            missing_deny.len()
        );
        for f in &missing_deny {
            eprintln!("  {f}");
        }
        eprintln!(
            "\n`mako_service::Json` refuses an undeclared key at runtime, except where serde\n\
             buffers — a flattened field, an untagged or internally tagged enum — and for\n\
             a body extracted past it. Add `#[serde(deny_unknown_fields)]` to the buffered\n\
             shape, replace a flatten by its fields, or list the use with its reason.\n\
             Without it a caller naming a field the API does not have gets a 2xx and the\n\
             value goes nowhere."
        );
    }
    if !undocumented_gate.is_empty() {
        eprintln!(
            "\nERROR: {} opaque-named field(s) documented as BO4E without a gate:\n",
            undocumented_gate.len()
        );
        for f in &undocumented_gate {
            eprintln!("  {f}");
        }
        eprintln!(
            "\nEither type the field `Bo4e<T>`, or decode it in the handler beside it —\n\
             or, if BO4E has no such type, stop the doc comment claiming it does."
        );
    }
    if !ungated.is_empty() {
        eprintln!(
            "\nERROR: {} BO4E document(s) in a request body typed `serde_json::Value`:\n",
            ungated.len()
        );
        for f in &ungated {
            eprintln!("  {f}");
        }
        eprintln!(
            "\nUse `mako_markt::bo4e::Bo4e<T>`. It runs the gate as serde deserialises —\n\
             `_typ`, schema, strict enums, BO4E's rules — and `canonical_json()` is what\n\
             to store. A `serde_json::Value` here is a document nothing checked."
        );
    }
    if !escapes.is_empty() {
        eprintln!(
            "\nERROR: {} comment(s) carrying a `\\uXXXX` escape:\n",
            escapes.len()
        );
        for f in &escapes {
            eprintln!("  {f}");
        }
        eprintln!(
            "\nWrite the character. `\\u00a742 EnWG` renders as `\\u00a742 EnWG` on\n\
             docs.rs, not as `§ 42 EnWG` — the escape is the language's inside a\n\
             string literal and nothing at all inside a comment."
        );
    }
    false
}

/// Does this line carry a `\uXXXX` escape?
///
/// Four hex digits after a literal backslash-`u`, which is the only shape a
/// JSON or Rust escape takes. Written out rather than pulled in as a regex
/// dependency — `xtask` has none and this is the whole grammar.
fn has_unicode_escape(line: &str) -> bool {
    let b = line.as_bytes();
    b.windows(2).enumerate().any(|(i, w)| {
        w == b"\\u"
            && b.get(i + 2..i + 6)
                .is_some_and(|d| d.iter().all(u8::is_ascii_hexdigit))
    })
}

// ── Scanning ──────────────────────────────────────────────────────────────────

/// Does the name say "this is a request body"?
///
/// Body types are found from the handler signature, which covers every one that
/// axum extracts directly. This adds the ones reached one level in — a
/// `pg::CreateKundeInput` behind a wrapper, a `*Body` nested in a request —
/// where the gate matters just as much and the signature does not name it.
fn names_a_request(name: &str) -> bool {
    ["Request", "Input", "Body", "Req"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// A `_json` suffix and a leading qualifier are noise: `kosten_json` is a
/// `Kosten` and `standort_adresse` is an `Adresse`.
fn names_a_bo4e_type(field: &str) -> bool {
    let base = field.strip_suffix("_json").unwrap_or(field);
    let last = base.rsplit('_').next().unwrap_or(base);
    BO4E_FIELD_NAMES.contains(&base) || BO4E_FIELD_NAMES.contains(&last)
}

/// Every `serde_json::Value` field whose doc comment mentions BO4E, as
/// `(field, doc, 1-based line)`.
///
/// Line-based for the same reason [`structs`] is: `cargo fmt` puts a doc line,
/// an attribute and a field declaration each on their own line.
fn bo4e_documented_value_fields(src: &str) -> Vec<(String, String, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("pub ") else {
            continue;
        };
        let Some((name, ty)) = rest.split_once(':') else {
            continue;
        };
        if !ty.contains("serde_json::Value") {
            continue;
        }
        let name = name.trim();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
            continue;
        }
        // Walk back over attributes to the doc block.
        let mut doc = Vec::new();
        let mut j = i;
        while j > 0 {
            j -= 1;
            let p = lines[j].trim_start();
            if p.starts_with("#[") {
                continue;
            }
            if let Some(d) = p.strip_prefix("///") {
                doc.push(d.trim());
                continue;
            }
            break;
        }
        if doc.is_empty() {
            continue;
        }
        doc.reverse();
        let doc = doc.join(" ");
        let lower = doc.to_ascii_lowercase();
        if lower.contains("bo4e") {
            out.push((name.to_owned(), doc, i + 1));
        }
    }
    out
}

/// The type named by a `` BO4E `Ident` `` claim in `doc`, when BO4E does not
/// define it.
///
/// Only that exact shape — the word BO4E immediately followed by a backticked
/// CamelCase identifier — because that is the claim, and anything looser reads
/// every backticked word in a long doc block as a type. `STROM` and `RECHNUNG`
/// are excluded by the CamelCase test: they are wire values, not type names.
///
/// The catalogue is [`BO4E_FIELD_NAMES`] (the one rule 2 uses, so the two
/// cannot disagree) plus [`BO4E_TYPES_NO_FIELD_IS_NAMED_FOR`].
fn names_an_unknown_bo4e_type(doc: &str) -> Option<String> {
    let mut rest = doc;
    while let Some(i) = rest.find("BO4E") {
        let after = rest[i + 4..].trim_start();
        rest = &rest[i + 4..];
        let Some(tail) = after.strip_prefix('`') else {
            continue;
        };
        let Some(close) = tail.find('`') else {
            continue;
        };
        let ident = &tail[..close];
        // CamelCase: alphabetic, initial capital, and at least one lowercase —
        // which is what separates a type name from a wire value.
        if ident.len() < 4
            || !ident.chars().all(|c| c.is_ascii_alphabetic())
            || !ident.starts_with(|c: char| c.is_ascii_uppercase())
            || !ident.chars().any(|c| c.is_ascii_lowercase())
        {
            continue;
        }
        let lower = ident.to_ascii_lowercase();
        if BO4E_FIELD_NAMES.contains(&lower.as_str())
            || BO4E_TYPES_NO_FIELD_IS_NAMED_FOR.contains(&lower.as_str())
        {
            continue;
        }
        return Some(ident.to_owned());
    }
    None
}

/// BO4E types the schema defines that no mako field is named for, so
/// [`BO4E_FIELD_NAMES`] does not list them and rule 4 would otherwise call them
/// invented.
const BO4E_TYPES_NO_FIELD_IS_NAMED_FOR: &[&str] = &[
    "aufabschlag",
    "ausschreibung",
    "betrag",
    "buendelvertrag",
    "energieherkunft",
    "konfigurationsprodukt",
    "kontaktweg",
    "lastvariablepreisposition",
    "lokationszuordnung",
    "menge",
    "preis",
    "preisblattmessung",
    "preisblattnetznutzung",
    "preismodell",
    "preisstaffel",
    "region",
    "steuerbareressource",
    "steuerbetrag",
    "tarif",
    "tarifkosten",
    "technischeressource",
    "zaehlwerk",
    "zeitraum",
    "zeitvariablepreisposition",
];

/// Does `doc` explicitly say the field is **not** BO4E?
///
/// `marktd`'s Tranche payload says so at length — BO4E defines no Tranche
/// Geschäftsobjekt — and a disclaimer has to mention BO4E to make it, so the
/// mention alone cannot be what triggers the rule.
fn disclaims_bo4e(doc: &str) -> bool {
    let flat = doc.replace('*', "").to_ascii_lowercase();
    ["not bo4e", "no bo4e", "not a bo4e", "is not bo4e"]
        .iter()
        .any(|n| flat.contains(n))
}

/// Close a set of body-type names over the field types they hold — Rule 2's
/// scope.
///
/// A request body is not only the type axum extracts: every struct reachable
/// from it by a field is deserialised from the same untrusted document. Rule 2
/// then narrows to the structs declared in `services/`, because a domain-crate
/// type reached by a body is also built in Rust, stored as JSONB and read back,
/// and that read is where a type change does harm.
fn reachable_body_types(seed: &[String], files: &[(String, String)]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    // Every definition of a name is followed, not the first one found: which
    // file comes first is the filesystem's `read_dir` order, so picking one
    // would make the verdict differ between macOS and Linux (`PartnerRecord` exists
    // in both `mako-engine` and `mako-markt`). Following all of them errs in
    // the conservative direction, as the attribute check below already does.
    let mut defs: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (_, src) in files {
        for s in structs(src) {
            defs.entry(s.name).or_default().extend(s.fields);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = seed.to_vec();
    while let Some(name) = stack.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(fields) = defs.get(&name) else {
            continue;
        };
        for (_, ty) in fields {
            let inner = innermost_type(ty);
            if defs.contains_key(&inner) && !seen.contains(&inner) {
                stack.push(inner);
            }
        }
    }
    let mut out: Vec<String> = seen.into_iter().collect();
    out.sort();
    out
}

/// Peel `Option`/`Vec`/`Box`/map wrappers and any path, leaving the bare name.
fn innermost_type(ty: &str) -> String {
    let mut t = ty.trim();
    loop {
        let stripped = [
            "Option<",
            "Vec<",
            "Box<",
            "HashMap<String, ",
            "BTreeMap<String, ",
        ]
        .iter()
        .find_map(|w| t.strip_prefix(*w));
        match stripped {
            Some(rest) => t = rest.strip_suffix('>').unwrap_or(rest).trim(),
            None => break,
        }
    }
    let head = t.split('<').next().unwrap_or(t);
    head.rsplit("::").next().unwrap_or(head).trim().to_owned()
}

/// How a handler reads its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// `mako_service::Json` — refuses an undeclared key at runtime.
    Strict,
    /// `mako_service::Lenient` — reads without the refusal, on purpose.
    Lenient,
    /// `axum::Json` — `serde` alone, no refusal.
    Axum,
}

/// One body extraction in a handler signature.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Extraction {
    file: String,
    ty: String,
    via: Via,
}

/// Does `src` bring `axum`'s `Json` into scope (outside test modules)?
fn imports_axum_json(src: &str) -> bool {
    let mut rest = src;
    while let Some(i) = rest.find("use axum") {
        let stmt = &rest[i..];
        let end = stmt.find(';').unwrap_or(stmt.len());
        let words = stmt[..end].split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        if words.into_iter().any(|w| w == "Json") {
            return true;
        }
        rest = &stmt[end..];
    }
    false
}

/// Every typed body extraction — `Json<T>`, `Lenient<T>`, `axum::Json<T>` — in
/// a handler signature or closure, de-generic'd. A `serde_json::Value` body
/// declares no field and is left out.
fn extractions(files: &[(String, String)]) -> Vec<Extraction> {
    let mut out: Vec<Extraction> = Vec::new();
    for (rel, src) in files {
        let plain = if imports_axum_json(src) {
            Via::Axum
        } else {
            Via::Strict
        };
        for (needle, via) in [
            ("): axum::Json<", Via::Axum),
            ("): mako_service::Json<", Via::Strict),
            ("): Json<", plain),
            ("): Lenient<", Via::Lenient),
            ("): mako_service::Lenient<", Via::Lenient),
        ] {
            let mut rest = src.as_str();
            while let Some(i) = rest.find(needle) {
                let after = &rest[i + needle.len()..];
                let Some(end) = after.find('>') else { break };
                let mut ty = after[..end].trim();
                ty = ty.strip_prefix("Vec<").unwrap_or(ty);
                let ty = ty.rsplit("::").next().unwrap_or(ty).trim();
                let e = Extraction {
                    file: rel.clone(),
                    ty: ty.to_owned(),
                    via,
                };
                if ty != "Value" && !ty.is_empty() && !out.contains(&e) {
                    out.push(e);
                }
                rest = after;
            }
        }
    }
    out
}

/// Every type named as a body in a handler signature, de-generic'd, however
/// it is extracted.
fn json_body_types(files: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in extractions(files) {
        if !out.contains(&e.ty) {
            out.push(e.ty);
        }
    }
    out
}

/// Rule 1: the body shapes the unknown-key refusal does not reach, unlisted.
fn rule_one(found: &[Extraction], files: &[(String, String)]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let mut out = Vec::new();

    // Extractions past the refusal: listed, and every listing still true.
    for (via, list, what) in [
        (Via::Lenient, LENIENT_USES, "LENIENT_USES"),
        (Via::Axum, AXUM_JSON_USES, "AXUM_JSON_USES"),
    ] {
        for e in found.iter().filter(|e| e.via == via) {
            if !list.iter().any(|(f, t, _)| *f == e.file && *t == e.ty) {
                out.push(format!(
                    "{}  {}: extracted with {} — past the unknown-key refusal; take \
                     `mako_service::Json`, or list it in {what} with its reason",
                    e.file,
                    e.ty,
                    if via == Via::Axum {
                        "axum::Json"
                    } else {
                        "Lenient"
                    }
                ));
            }
        }
        for (f, t, _) in list {
            if !found
                .iter()
                .any(|e| e.via == via && e.file == *f && e.ty == *t)
            {
                out.push(format!(
                    "{f}  {t}: listed in {what}, and no such extraction remains"
                ));
            }
        }
    }

    // The shapes serde buffers, walked from every strict body.
    let mut struct_defs: HashMap<String, Vec<(String, StructDef)>> = HashMap::new();
    let mut enum_defs: HashMap<String, Vec<(String, EnumDef)>> = HashMap::new();
    for (rel, src) in files {
        for d in structs(src) {
            struct_defs
                .entry(d.name.clone())
                .or_default()
                .push((rel.clone(), d));
        }
        for d in enums(src) {
            enum_defs
                .entry(d.name.clone())
                .or_default()
                .push((rel.clone(), d));
        }
    }
    // Where a type is read from: in the runtime refusal's sight, out of a
    // buffer (the flattening struct or the enum that opened it), or out of a
    // buffer BUFFER_EXEMPT lists — whose whole subtree is that entry's claim.
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
    enum Seen {
        Checked,
        Exempt,
        Buffered(String),
    }
    let listed = |name: &str| BUFFER_EXEMPT.iter().any(|(_, n, _)| *n == name);
    let mut reached: HashMap<String, Seen> = HashMap::new();
    let mut stack: Vec<(String, Seen)> = found
        .iter()
        .filter(|e| e.via == Via::Strict)
        .map(|e| (e.ty.clone(), Seen::Checked))
        .collect();
    while let Some((name, seen)) = stack.pop() {
        // A stronger reading replaces a weaker one — buffered over exempt over
        // checked — so a type an exemption covers on one path is still held
        // where another path reads it out of an unlisted buffer.
        if reached.get(&name).is_some_and(|was| *was >= seen) {
            continue;
        }
        reached.insert(name.clone(), seen.clone());
        let open = |by: &str, opens: bool| -> Seen {
            match &seen {
                Seen::Checked if opens && listed(by) => Seen::Exempt,
                Seen::Checked if opens => Seen::Buffered(by.to_owned()),
                other => other.clone(),
            }
        };
        for (_, d) in struct_defs.get(&name).into_iter().flatten() {
            for (field, ty) in &d.fields {
                stack.push((
                    innermost_type(ty),
                    open(&d.name, d.flattened.contains(field)),
                ));
            }
        }
        for (_, d) in enum_defs.get(&name).into_iter().flatten() {
            let child = open(&d.name, d.buffers());
            for ty in &d.inner {
                stack.push((innermost_type(ty), child.clone()));
            }
        }
    }
    let mut flagged: HashSet<String> = HashSet::new();
    let mut names: Vec<_> = reached.iter().collect();
    names.sort_by(|a, b| a.0.cmp(b.0));
    for (name, seen) in names {
        let opened_by = match seen {
            Seen::Exempt => continue,
            Seen::Buffered(by) => Some(by.as_str()),
            Seen::Checked => None,
        };
        for (rel, d) in struct_defs.get(name).into_iter().flatten() {
            if !d.attrs.contains("Deserialize") {
                continue;
            }
            if !d.flattened.is_empty() && !listed(&d.name) {
                out.push(format!(
                    "{rel}  {}: flattens `{}` — a key left over beside it is unseen; declare \
                     the fields instead of flattening, or list it in BUFFER_EXEMPT",
                    d.name,
                    d.flattened.join("`, `")
                ));
            }
            if let Some(by) = opened_by
                && !d.attrs.contains("deny_unknown_fields")
                && flagged.insert(format!("{rel}{}", d.name))
            {
                out.push(format!(
                    "{rel}  {}: read out of `{by}`'s buffer without `deny_unknown_fields`",
                    d.name
                ));
            }
        }
        for (rel, d) in enum_defs.get(name).into_iter().flatten() {
            // A unit or newtype variant has no key of its own; a newtype's
            // member is walked, and held, as a type of its own.
            if !d.attrs.contains("Deserialize")
                || d.attrs.contains("deny_unknown_fields")
                || !d.struct_variants
                || listed(&d.name)
            {
                continue;
            }
            if (opened_by.is_some() || d.buffers()) && flagged.insert(format!("{rel}{}", d.name)) {
                out.push(format!(
                    "{rel}  {}: {} without `deny_unknown_fields`",
                    d.name,
                    match opened_by {
                        Some(by) if !d.buffers() => format!("an enum read out of `{by}`'s buffer"),
                        _ => "an untagged or internally tagged enum".to_owned(),
                    }
                ));
            }
        }
    }
    for (f, n, _) in BUFFER_EXEMPT {
        let flattens = struct_defs
            .get(*n)
            .into_iter()
            .flatten()
            .any(|(rel, d)| rel == f && !d.flattened.is_empty());
        let buffers = enum_defs
            .get(*n)
            .into_iter()
            .flatten()
            .any(|(rel, d)| rel == f && d.buffers());
        if !reached.contains_key(*n) || !(flattens || buffers) {
            out.push(format!(
                "{f}  {n}: listed in BUFFER_EXEMPT, and no body reaches a buffer there"
            ));
        }
    }
    out
}

struct EnumDef {
    name: String,
    attrs: String,
    /// Every type a variant holds — tuple-variant members and struct-variant
    /// field types.
    inner: Vec<String>,
    /// Whether a variant declares named fields — the only keys the enum's own
    /// `deny_unknown_fields` governs.
    struct_variants: bool,
}

impl EnumDef {
    /// `untagged`, or internally tagged (`tag` without `content`): the two
    /// enum shapes `serde` reads out of its own buffer.
    fn buffers(&self) -> bool {
        let serde: String = self
            .attrs
            .lines()
            .filter(|l| l.contains("serde("))
            .collect::<Vec<_>>()
            .join(" ");
        serde.contains("untagged") || (serde.contains("tag =") && !serde.contains("content ="))
    }
}

/// Every `enum Name { … }` with its attribute block and the types its
/// variants hold. Line-based, like [`structs`].
fn enums(src: &str) -> Vec<EnumDef> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let is_decl = t.starts_with("enum ")
            || t.starts_with("pub enum ")
            || (t.starts_with("pub(") && t.contains(") enum "));
        if !is_decl || !lines[i].trim_end().ends_with('{') {
            i += 1;
            continue;
        }
        let Some(name) = lines[i]
            .split("enum ")
            .nth(1)
            .and_then(|r| r.split(|c: char| c.is_whitespace() || c == '<').next())
            .map(|n| n.trim_end_matches('{').trim().to_owned())
        else {
            i += 1;
            continue;
        };
        let mut start = i;
        while start > 0 {
            let p = lines[start - 1].trim_start();
            if p.starts_with("#[") || p.starts_with("///") || p.starts_with(')') {
                start -= 1;
            } else if !p.starts_with("//") && (p.ends_with(',') || p.starts_with('"')) {
                // A wrapped attribute argument line.
                start -= 1;
            } else {
                break;
            }
        }
        let attrs = lines[start..i].join("\n");
        let indent = lines[i].len() - lines[i].trim_start().len();
        let mut inner = Vec::new();
        let mut struct_variants = false;
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if l.trim() == "}" && (l.len() - l.trim_start().len()) == indent {
                break;
            }
            let v = l.trim();
            if v.starts_with("//") || v.starts_with('#') {
                j += 1;
                continue;
            }
            if let Some(open) = v.find('(')
                && v[..open]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                let close = v.rfind(')').unwrap_or(v.len());
                for ty in split_top_level(&v[open + 1..close.max(open + 1)]) {
                    inner.push(ty);
                }
            } else if let Some((field, ty)) = v.split_once(':')
                && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !field.is_empty()
            {
                inner.push(ty.trim().trim_end_matches(',').to_owned());
            } else if v.ends_with('{') {
                struct_variants = true;
            }
            j += 1;
        }
        out.push(EnumDef {
            name,
            attrs,
            inner,
            struct_variants,
        });
        i = j;
    }
    out
}

/// `a, B<c, d>, e` → `["a", "B<c, d>", "e"]`.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_owned());
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

struct StructDef {
    name: String,
    attrs: String,
    fields: Vec<(String, String)>,
    /// The fields carrying `#[serde(flatten)]`.
    flattened: Vec<String>,
}

/// Every `struct Name { … }` with its preceding attribute block and its fields.
///
/// Deliberately line-based rather than a parser: the shapes this reads are
/// `#[derive(...)]`, `#[serde(...)]` and `pub name: Type,`, and every one of
/// them is written on its own line in this tree (`cargo fmt` guarantees it).
fn structs(src: &str) -> Vec<StructDef> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let is_decl = t.starts_with("struct ")
            || t.starts_with("pub struct ")
            || (t.starts_with("pub(") && t.contains(") struct "));
        if !is_decl || !lines[i].trim_end().ends_with('{') {
            i += 1;
            continue;
        }
        let Some(name) = lines[i]
            .split("struct ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .map(|n| n.trim_end_matches('{').trim().to_owned())
        else {
            i += 1;
            continue;
        };
        // Walk back over the attribute/doc block.
        let mut start = i;
        while start > 0 {
            let p = lines[start - 1].trim_start();
            if p.starts_with("#[") || p.starts_with("///") || p.starts_with("//!") || p.is_empty() {
                if p.is_empty()
                    && !lines[..start - 1].iter().rev().any(|l| {
                        let l = l.trim_start();
                        l.starts_with("#[") || l.starts_with("///")
                    })
                {
                    break;
                }
                start -= 1;
            } else {
                break;
            }
        }
        let attrs = lines[start..i].join("\n");
        // Fields, to the closing brace at the declaration's indent.
        let indent = lines[i].len() - lines[i].trim_start().len();
        let mut fields = Vec::new();
        let mut flattened = Vec::new();
        let mut pending_flatten = false;
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if l.trim() == "}" && (l.len() - l.trim_start().len()) == indent {
                break;
            }
            let ft = l.trim_start();
            if ft.starts_with("#[") && ft.contains("serde(") && ft.contains("flatten") {
                pending_flatten = true;
            }
            if let Some(rest) = ft
                .strip_prefix("pub ")
                .or_else(|| (!ft.starts_with('#') && !ft.starts_with("//")).then_some(ft))
                && let Some((name, ty)) = rest.split_once(':')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.is_empty()
            {
                fields.push((
                    name.trim().to_owned(),
                    ty.trim().trim_end_matches(',').to_owned(),
                ));
                if std::mem::take(&mut pending_flatten) {
                    flattened.push(name.trim().to_owned());
                }
            }
            j += 1;
        }
        out.push(StructDef {
            name,
            attrs,
            fields,
            flattened,
        });
        i = j;
    }
    out
}

/// Everything outside a `#[cfg(test)]` module.
///
/// A test may declare a body type that deliberately breaks a rule — the
/// extractor's own rejection tests do exactly that — and holding a test to a
/// production rule would make the rule untestable.
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            // Skip the attribute, then the item it guards, brace-counting from
            // the first `{` so a nested block cannot end it early.
            let mut depth = 0i32;
            let mut started = false;
            for l in lines.by_ref() {
                depth += i32::try_from(l.matches('{').count()).unwrap_or(0);
                if depth > 0 {
                    started = true;
                }
                depth -= i32::try_from(l.matches('}').count()).unwrap_or(0);
                if started && depth <= 0 {
                    break;
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn collect_rs(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_rs(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        // Tests hand-write payloads on purpose; a fixture is untrusted input by
        // definition and is exactly what these rules exist to refuse.
        if rel.contains("/tests/") || rel.ends_with("/tests.rs") {
            continue;
        }
        if let Ok(src) = std::fs::read_to_string(&path) {
            out.push((rel, strip_test_modules(&src)));
        }
    }
}

#[cfg(test)]
mod tests {

    /// A scan that reaches no file has checked nothing, and must say so rather
    /// than report the clean line.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!super::run(std::path::Path::new(
            "/nonexistent/mako/workspace/root"
        )));
    }
    use super::{
        has_unicode_escape, json_body_types, names_a_bo4e_type, strip_test_modules, structs,
    };

    /// A body field typed from a workspace crate is reached, so the crate type
    /// is held to rule 1 too: `GridInput` lives in `energy-billing`, and a
    /// mistyped grid override in a `billingd` body would otherwise bill no
    /// Netzentgelt.
    #[test]
    fn a_crate_type_held_by_a_service_body_is_reached() {
        let files = vec![
            (
                "services/billingd/src/handlers/calculate.rs".to_owned(),
                "#[derive(Deserialize)]\npub struct CalculateRequest {\n    pub grid: Option<GridInput>,\n}\n"
                    .to_owned(),
            ),
            (
                "crates/energy-billing/src/quantities.rs".to_owned(),
                "#[derive(Deserialize)]\npub struct GridInput {\n    pub nne: Option<Rates>,\n}\n\
                 #[derive(Deserialize)]\npub struct Rates {\n    pub ap: u32,\n}\n"
                    .to_owned(),
            ),
        ];
        let reached = super::reachable_body_types(&["CalculateRequest".to_owned()], &files);
        assert!(reached.contains(&"GridInput".to_owned()), "{reached:?}");
        assert!(
            reached.contains(&"Rates".to_owned()),
            "transitively: {reached:?}"
        );
    }

    /// Two crates defining the same name: both definitions are followed, in
    /// either file order, so the verdict cannot depend on `read_dir`.
    #[test]
    fn every_same_named_definition_is_followed_in_any_order() {
        let a = (
            "crates/mako-markt/src/repository.rs".to_owned(),
            "#[derive(Deserialize)]\npub struct PartnerRecord {\n    pub notes: String,\n}\n"
                .to_owned(),
        );
        let b = (
            "crates/mako-engine/src/partner.rs".to_owned(),
            "#[derive(Deserialize)]\npub struct PartnerRecord {\n    pub contacts: Vec<ContactPerson>,\n}\n\
             #[derive(Deserialize)]\npub struct ContactPerson {\n    pub name: String,\n}\n"
                .to_owned(),
        );
        for files in [vec![a.clone(), b.clone()], vec![b, a]] {
            let reached = super::reachable_body_types(&["PartnerRecord".to_owned()], &files);
            assert!(reached.contains(&"ContactPerson".to_owned()), "{reached:?}");
        }
    }

    #[test]
    fn a_bo4e_field_name_is_recognised_through_its_noise() {
        for f in [
            "vertrag",
            "kosten_json",
            "standort_adresse",
            "geschaeftspartner",
            "fremdkosten_json",
        ] {
            assert!(names_a_bo4e_type(f), "{f} names a BO4E type");
        }
        for f in ["data", "payload", "view", "evidence", "fulfillment_data"] {
            assert!(!names_a_bo4e_type(f), "{f} names no BO4E type");
        }
    }

    #[test]
    fn the_scanner_reads_attributes_and_fields() {
        let src = "\
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Req {
    /// doc
    pub vertrag: Option<serde_json::Value>,
    pub n: u8,
}
";
        let defs = structs(src);
        assert_eq!(defs.len(), 1);
        assert!(defs[0].attrs.contains("deny_unknown_fields"));
        assert_eq!(defs[0].fields.len(), 2);
        assert_eq!(defs[0].fields[0].0, "vertrag");
    }

    /// A `#[cfg(test)]` module is not production surface, and the extractor's
    /// own tests declare a body type that breaks rule 1 on purpose.
    #[test]
    fn an_escape_is_found_only_where_it_is_wrong() {
        assert!(has_unicode_escape(r"/// Optional \u00a742 EnWG payload"));
        assert!(has_unicode_escape(r"// VNB \u2192 UENB"));
        assert!(!has_unicode_escape("/// Optional § 42 EnWG payload"));
        // Not four hex digits, so not an escape.
        assert!(!has_unicode_escape(r"/// the \under_score convention"));
    }

    #[test]
    fn test_modules_are_not_scanned() {
        let src = "\
pub struct Kept { pub a: u8 }
#[cfg(test)]
mod tests {
    #[derive(Deserialize)]
    pub struct Hidden { pub vertrag: serde_json::Value }
}
pub struct AlsoKept { pub b: u8 }
";
        let stripped = strip_test_modules(src);
        assert!(stripped.contains("Kept"));
        assert!(stripped.contains("AlsoKept"));
        assert!(!stripped.contains("Hidden"), "{stripped}");
    }

    /// Rule 1 over a synthetic tree, without the real lists' staleness
    /// findings (those name files the synthetic tree does not have).
    fn rule_one_on(files: &[(&str, &str)]) -> Vec<String> {
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(f, s)| ((*f).to_owned(), (*s).to_owned()))
            .collect();
        super::rule_one(&super::extractions(&files), &files)
            .into_iter()
            .filter(|f| !f.contains("listed in"))
            .collect()
    }

    /// The runtime refusal holds a plain body: a type without
    /// `deny_unknown_fields` is no finding, because `mako_service::Json`
    /// answers an undeclared key with a 422 naming it
    /// (`an_undeclared_key_is_refused_with_its_path`).
    #[test]
    fn a_plain_body_without_the_attribute_is_the_extractors_business() {
        let out = rule_one_on(&[(
            "services/a/src/h.rs",
            "async fn h(Json(r): Json<Req>) {}\n\
             #[derive(Deserialize)]\npub struct Req {\n    pub inner: Inner,\n}\n\
             #[derive(Deserialize)]\npub struct Inner {\n    pub n: u8,\n}\n",
        )]);
        assert!(out.is_empty(), "{out:?}");
    }

    /// A flattened field is a buffer: the flattening struct is named, and so
    /// is a member read out of it without the attribute.
    #[test]
    fn a_flatten_is_named_and_its_member_must_deny() {
        let out = rule_one_on(&[(
            "services/a/src/h.rs",
            "async fn h(Json(r): Json<Req>) {}\n\
             #[derive(Deserialize)]\npub struct Req {\n    #[serde(flatten)]\n    pub inner: Inner,\n}\n\
             #[derive(Deserialize)]\npub struct Inner {\n    pub n: u8,\n}\n",
        )]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(
            out.iter().any(|f| f.contains("Req: flattens `inner`")),
            "{out:?}"
        );
        assert!(
            out.iter()
                .any(|f| f.contains("Inner: read out of `Req`'s buffer")),
            "{out:?}"
        );
    }

    /// An internally tagged enum with named-field variants denies, and the
    /// struct a newtype variant holds is read out of its buffer.
    #[test]
    fn a_tagged_enum_and_what_it_holds_must_deny() {
        let src = "async fn h(Json(r): Json<Req>) {}\n\
             #[derive(Deserialize)]\npub struct Req {\n    pub art: Art,\n}\n\
             #[derive(Deserialize)]\n#[serde(tag = \"art\")]\npub enum Art {\n    A(Teil),\n    B {\n        n: u8,\n    },\n}\n\
             #[derive(Deserialize)]\npub struct Teil {\n    pub m: u8,\n}\n";
        let out = rule_one_on(&[("services/a/src/h.rs", src)]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(
            out.iter()
                .any(|f| f.contains("Art: an untagged or internally tagged")),
            "{out:?}"
        );
        assert!(
            out.iter()
                .any(|f| f.contains("Teil: read out of `Art`'s buffer")),
            "{out:?}"
        );

        let fixed = src
            .replace(
                "#[serde(tag = \"art\")]",
                "#[serde(tag = \"art\", deny_unknown_fields)]",
            )
            .replace(
                "#[derive(Deserialize)]\npub struct Teil",
                "#[derive(Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct Teil",
            );
        assert!(rule_one_on(&[("services/a/src/h.rs", &fixed)]).is_empty());
    }

    /// A body read past the refusal — `Lenient<T>` or `axum::Json<T>` — is
    /// named until it is listed.
    #[test]
    fn an_extraction_past_the_refusal_is_named() {
        let files = vec![
            (
                "services/a/src/h.rs".to_owned(),
                "async fn h(Lenient(r): Lenient<Push>) {}\n".to_owned(),
            ),
            (
                "services/b/src/h.rs".to_owned(),
                "use axum::{Json, Router};\nasync fn h(Json(r): Json<Ident>) {}\n".to_owned(),
            ),
        ];
        let found = super::extractions(&files);
        assert!(
            found
                .iter()
                .any(|e| e.ty == "Push" && e.via == super::Via::Lenient)
        );
        assert!(
            found
                .iter()
                .any(|e| e.ty == "Ident" && e.via == super::Via::Axum)
        );
        let out = super::rule_one(&found, &files);
        assert!(
            out.iter()
                .any(|f| f.contains("Push: extracted with Lenient")),
            "{out:?}"
        );
        assert!(
            out.iter()
                .any(|f| f.contains("Ident: extracted with axum::Json")),
            "{out:?}"
        );
    }

    /// Today's tree: no buffered shape unlisted, every listed use still there.
    #[test]
    fn the_real_tree_has_no_unlisted_blind_spot() {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let mut files = Vec::new();
        for dir in ["crates", "services"] {
            super::collect_rs(&root.join(dir), root, &mut files);
        }
        let out = super::rule_one(&super::extractions(&files), &files);
        assert!(out.is_empty(), "{out:#?}");
    }

    #[test]
    fn body_types_come_off_the_handler_signature() {
        let files = vec![(
            "x.rs".to_owned(),
            "async fn h(Json(req): Json<pg::UpsertThing>) {}\n\
             async fn g(Json(b): Json<serde_json::Value>) {}\n"
                .to_owned(),
        )];
        assert_eq!(json_body_types(&files), vec!["UpsertThing".to_owned()]);
    }
}
