//! Guard: a JSON request body refuses what it cannot store, and gates what is
//! BO4E.
//!
//! Two rules, one scan, because both are about the same moment — the instant an
//! untrusted body becomes a Rust value — and both failed the same way: silently.
//!
//! ## Rule 1 — every `Json<T>` body type denies unknown fields
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
//! `#[serde(deny_unknown_fields)]` makes it a `422` naming the field — well-formed
//! JSON that the schema refuses, which is what 422 is for.
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
//! Each is a claim that has to be argued in writing, so both lists carry a
//! reason per entry and both are short.

use std::path::Path;

/// Body types that may not deny unknown fields, with the reason.
///
/// Keyed by name **and** the file that declares it: two different services
/// have an `UpsertRequest`, and a name-only key exempted the one that has no
/// reason to be exempt along with the one that does.
const DENY_EXEMPT: &[(&str, &str, &str)] = &[
    (
        "services/outputd/src/handlers.rs",
        "IssueDocumentRequest",
        "carries a `#[serde(flatten)]` field; serde rejects the combination",
    ),
    (
        "services/outputd/src/handlers.rs",
        "RenderApiRequest",
        "carries a `#[serde(flatten)]` field; serde rejects the combination",
    ),
    (
        "services/edmd/src/server/lastgang.rs",
        "SmgwTyp2Push",
        "BSI TR-03109 defines this payload, not mako — a conformant \
         Smart-Meter-Gateway one revision ahead must be read, not refused",
    ),
    // The four below are BDEW **Energy API** types (MaLo-Identifikation, the
    // API-Verzeichnis). The schema is the BDEW's, and forward compatibility is
    // its property, not mako's: a counterparty on a later minor version adds
    // fields, and refusing their whole document over one would be mako failing
    // an interoperability requirement it does not get to redefine.
    (
        "crates/energy-api/src/models/electricity.rs",
        "IdentificationParameter",
        "BDEW Energy API request schema — a later minor version may add fields",
    ),
    (
        "crates/energy-api/src/models/electricity.rs",
        "MaloIdentResultPositive",
        "BDEW Energy API response schema — a later minor version may add fields",
    ),
    (
        "crates/mako-markt/src/makod_client.rs",
        "MaloIdentResultPositive",
        "the client-side mirror of the same BDEW Energy API response",
    ),
    (
        "crates/energy-api/src/models/electricity.rs",
        "MaloIdentResultNegative",
        "BDEW Energy API response schema — a later minor version may add fields",
    ),
    (
        "crates/energy-api/src/models/directory.rs",
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

/// Run both rules. Returns `true` when the tree is clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    for dir in ["crates", "services"] {
        collect_rs(&workspace_root.join(dir), workspace_root, &mut files);
    }

    let body_types = json_body_types(&files);
    let mut missing_deny = Vec::new();
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
            // Body types are matched by name across the tree, so a *client-side*
            // request builder that happens to share a name with an inbound body
            // — `accountingd`'s `IssueDocumentRequest`, which it serialises and
            // posts to `outputd` — would be flagged for a rule about
            // deserialising. A struct that does not derive `Deserialize` is not
            // something anyone can send.
            // Matched by **name**, so a same-named type elsewhere is held to the
            // rule too — `mako_markt::PartnerRecord` alongside `makod`'s. That
            // errs in the conservative direction: a record that absorbs unknown
            // keys in silence is not better for being a read row. Rule 2, whose
            // remedy is a type change, is scoped more tightly below.
            let is_body = body_types.contains(&s.name) && s.attrs.contains("Deserialize");
            if is_body
                && !s.attrs.contains("deny_unknown_fields")
                && !DENY_EXEMPT
                    .iter()
                    .any(|(file, n, _)| *n == s.name && rel == file)
            {
                missing_deny.push(format!("{rel}  {}", s.name));
            }
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
            "check-request-bodies: {} JSON body type(s) deny unknown fields, \
             no ungated BO4E field, no escaped comment ({} + {} documented exemption(s))",
            body_types.len(),
            DENY_EXEMPT.len(),
            GATE_EXEMPT.len()
        );
        return true;
    }

    if !missing_deny.is_empty() {
        eprintln!(
            "ERROR: {} JSON request body type(s) accept fields they do not have:\n",
            missing_deny.len()
        );
        for f in &missing_deny {
            eprintln!("  {f}");
        }
        eprintln!(
            "\nAdd `#[serde(deny_unknown_fields)]`. Without it a caller naming a field\n\
             the API does not have gets a 2xx and the value goes nowhere — a customer\n\
             created with no name, and an invoice issued to nobody."
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

/// Every type named as `Json<T>` in a handler signature, de-generic'd.
fn json_body_types(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, src) in files {
        let mut rest = src.as_str();
        while let Some(i) = rest.find("): Json<") {
            let after = &rest[i + "): Json<".len()..];
            let Some(end) = after.find('>') else { break };
            let mut ty = after[..end].trim();
            ty = ty.strip_prefix("Vec<").unwrap_or(ty);
            let ty = ty.rsplit("::").next().unwrap_or(ty).trim();
            if ty != "Value" && !ty.is_empty() && !out.iter().any(|t| t == ty) {
                out.push(ty.to_owned());
            }
            rest = after;
        }
    }
    out
}

struct StructDef {
    name: String,
    attrs: String,
    fields: Vec<(String, String)>,
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
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if l.trim() == "}" && (l.len() - l.trim_start().len()) == indent {
                break;
            }
            let ft = l.trim_start();
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
            }
            j += 1;
        }
        out.push(StructDef {
            name,
            attrs,
            fields,
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
    use super::{
        has_unicode_escape, json_body_types, names_a_bo4e_type, strip_test_modules, structs,
    };

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
