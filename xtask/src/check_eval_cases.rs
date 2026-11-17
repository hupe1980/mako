//! Guard: every `tool-calling` specialist has labelled eval cases, and no case
//! carries personal data.
//!
//! A specialist is measured only against a case whose right answer is known
//! (`services/agentd/evals/<specialist>/`), and the `evals` suite in agentd
//! replays those cases. A specialist with no case reads `not measured` in the
//! report, which is honest, and stays that way unnoticed, which is not. So:
//!
//! * every manifest whose `execution.kind` is `tool-calling` — the set is
//!   derived from the manifests, never a count — has at least one `positive`
//!   case and one `negative` or `insufficient-evidence` case, unless it is on
//!   [`WITHOUT_CASES`], a shrink-only ratchet: a specialist that gains cases
//!   fails until its entry goes, and an entry naming no tool-calling
//!   specialist fails;
//! * every case has its cassette and its recorded outcome, a known label and
//!   a provenance, and a twin names a clean case of the same specialist;
//! * no case or cassette carries an e-mail address, an IBAN or a person's
//!   name outside the synthetic ones: recording a case sends its text to a
//!   provider, so it must be synthetic before it is recorded, not after.
//!   MaLo-IDs in the tree are held to their check digit by `check-malo-ids`.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

const AGENTS: &str = "services/agentd/agents";
const EVALS: &str = "services/agentd/evals";

/// Tool-calling specialists without cases yet. Shrink-only.
const WITHOUT_CASES: &[&str] = &[
    "billing-agent",
    "billing-anomaly-agent",
    "billing-regulatory-guard-agent",
    "compliance-agent",
    "eeg-agent",
    "eeg-compliance-agent",
    "einrichtung-assistent",
    "einsd-batch-agent",
    "grid-anomaly-agent",
    "invoice-reconciliation-agent",
    "jahresabrechnung-agent",
    "mabis-agent",
    "mako-agent",
    "meter-data-agent",
    "msb-history-agent",
    "netzbilanz-agent",
    "payment-reconciliation-agent",
    "portald-agent",
    "processd-agent",
    "productd-agent",
    "regulatory-reporting-agent",
    "replacement-value-agent",
    "smgw-diagnostics-agent",
    "sperrd-agent",
    "tariff-optimization-agent",
    "vertragd-agent",
    "vpp-billing-agent",
];

/// The names a case may carry for a person.
const SYNTHETIC_NAMES: &[&str] = &[
    "Erika Mustermann",
    "Max Mustermann",
    "Erika",
    "Max",
    "Mustermann",
];

/// Object keys whose value is a person's name.
const NAME_KEYS: &[&str] = &[
    "vorname",
    "nachname",
    "kundenname",
    "ansprechpartner",
    "inhaber",
];

/// Entry point.
pub fn run(root: &Path) -> bool {
    let specialists = match tool_calling(&root.join(AGENTS)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("check-eval-cases: {e}");
            return false;
        }
    };
    if specialists.is_empty() {
        eprintln!("check-eval-cases: no tool-calling manifest under {AGENTS} — the layout changed");
        return false;
    }
    let findings = check(&specialists, &root.join(EVALS), WITHOUT_CASES);
    if findings.is_empty() {
        println!(
            "check-eval-cases: {} tool-calling specialists, {} with cases, {} on the ratchet",
            specialists.len(),
            specialists.len() - WITHOUT_CASES.len(),
            WITHOUT_CASES.len()
        );
        return true;
    }
    for f in &findings {
        eprintln!("check-eval-cases: {f}");
    }
    false
}

/// The `metadata.name` of every manifest whose `spec.execution.kind` is
/// `tool-calling`.
fn tool_calling(dir: &Path) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in rd.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "yaml") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Value =
            serde_saphyr::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if doc.pointer("/spec/execution/kind").and_then(Value::as_str) == Some("tool-calling") {
            let name = doc
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{}: no metadata.name", path.display()))?;
            out.insert(name.to_owned());
        }
    }
    Ok(out)
}

fn check(specialists: &BTreeSet<String>, evals: &Path, ratchet: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for entry in ratchet {
        if !specialists.contains(*entry) {
            findings.push(format!("WITHOUT_CASES names {entry}, which is no tool-calling specialist — remove the entry"));
        }
    }
    for name in specialists {
        let dir = evals.join(name);
        let (positive, negative) = cases(name, &dir, &mut findings);
        let has = positive > 0 && negative > 0;
        let listed = ratchet.contains(&name.as_str());
        match (has, listed) {
            (true, true) => findings.push(format!(
                "{name} has cases now — remove it from WITHOUT_CASES"
            )),
            (false, false) => findings.push(format!(
                "{name} needs ≥ 1 positive and ≥ 1 negative or insufficient-evidence case under {EVALS}/{name}/cases (has {positive} and {negative})"
            )),
            _ => {}
        }
    }
    if let Ok(rd) = std::fs::read_dir(evals) {
        for e in rd.filter_map(Result::ok) {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !specialists.contains(&n) {
                findings.push(format!(
                    "{EVALS}/{n}: no tool-calling specialist has this name"
                ));
            }
        }
    }
    findings
}

/// Check one specialist's cases; returns the positive and the
/// negative-or-escalation counts.
fn cases(name: &str, dir: &Path, findings: &mut Vec<String>) -> (usize, usize) {
    let Ok(rd) = std::fs::read_dir(dir.join("cases")) else {
        return (0, 0);
    };
    if !dir.join("scoring.json").is_file() {
        findings.push(format!("{name}: no scoring.json"));
    }
    let mut files: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    files.sort();
    let mut ids = BTreeSet::new();
    let mut twins = Vec::new();
    let (mut positive, mut negative) = (0, 0);
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let at = path.display().to_string();
        let Some(case) = read(path, findings) else {
            continue;
        };
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        if path.file_stem().and_then(|s| s.to_str()) != Some(id.as_str()) {
            findings.push(format!("{at}: id {id:?} is not the file name"));
        }
        if case["specialist"].as_str() != Some(name) {
            findings.push(format!("{at}: specialist is not {name}"));
        }
        match case.pointer("/expected/label").and_then(Value::as_str) {
            Some("positive") => positive += 1,
            Some("negative" | "insufficient-evidence") => negative += 1,
            other => findings.push(format!("{at}: label {other:?}")),
        }
        let provenance = case["provenance"].as_str().unwrap_or_default();
        let ok = provenance == "synthetic"
            || provenance.starts_with("labelled-by ")
            || provenance
                .strip_prefix("derived-from-fixture ")
                .is_some_and(|p| {
                    dir.ancestors()
                        .nth(4)
                        .is_some_and(|root| root.join(p).exists())
                });
        if !ok {
            findings.push(format!("{at}: provenance {provenance:?}"));
        }
        if let Some(clean) = case["twin_of"].as_str() {
            twins.push((at.clone(), clean.to_owned()));
        }
        personal_data(&at, &case, findings);
        for sub in ["cassettes", "expected"] {
            let p = dir.join(sub).join(format!("{id}.json"));
            match sub {
                "cassettes" => {
                    if let Some(c) = read(&p, findings) {
                        personal_data(&p.display().to_string(), &c, findings);
                    }
                }
                _ if !p.is_file() => findings.push(format!("{at}: no {sub}/{id}.json")),
                _ => {}
            }
        }
        ids.insert(id);
    }
    for (at, clean) in twins {
        if !ids.contains(&clean) {
            findings.push(format!("{at}: twin of {clean}, which is no case of {name}"));
        }
    }
    (positive, negative)
}

fn read(path: &Path, findings: &mut Vec<String>) -> Option<Value> {
    match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => Some(v),
        Err(e) => {
            findings.push(format!("{}: {e}", path.display()));
            None
        }
    }
}

/// E-mail addresses, IBANs and non-synthetic names anywhere in `v`.
fn personal_data(at: &str, v: &Value, findings: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            for word in s.split(|c: char| {
                c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '(' | ')' | '"' | '\'')
            }) {
                if looks_like_email(word) {
                    findings.push(format!("{at}: an e-mail address ({word})"));
                }
                if looks_like_iban(word) {
                    findings.push(format!("{at}: an IBAN ({word})"));
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| personal_data(at, x, findings)),
        Value::Object(m) => {
            for (k, x) in m {
                if NAME_KEYS.contains(&k.to_ascii_lowercase().as_str())
                    && let Some(n) = x.as_str()
                    && !SYNTHETIC_NAMES.contains(&n)
                {
                    findings.push(format!("{at}: {k} {n:?} is not a synthetic name"));
                }
                personal_data(at, x, findings);
            }
        }
        _ => {}
    }
}

fn looks_like_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && domain
            .rsplit('.')
            .next()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Two letters, two check digits, 11–30 alphanumerics: the IBAN shape
/// (ISO 13616). No test IBAN is admitted — a case needs none.
fn looks_like_iban(word: &str) -> bool {
    let b = word.as_bytes();
    (15..=34).contains(&b.len())
        && b[..2].iter().all(u8::is_ascii_uppercase)
        && b[2..4].iter().all(u8::is_ascii_digit)
        && b[4..]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && b[4..].iter().any(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("check-eval-cases-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tempdir");
        d
    }

    fn put(dir: &Path, rel: &str, v: &Value) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, serde_json::to_string(v).expect("json")).expect("write");
    }

    fn case(dir: &Path, id: &str, label: &str, text: &str) {
        put(
            dir,
            &format!("demo/cases/{id}.json"),
            &json!({
                "specialist": "demo", "id": id, "provenance": "synthetic",
                "expected": { "label": label, "findings": [] },
                "tool_results": { "x/y": [{ "note": text }] },
            }),
        );
        put(
            dir,
            &format!("demo/cassettes/{id}.json"),
            &json!({ "case": id, "turns": [] }),
        );
        put(dir, &format!("demo/expected/{id}.json"), &json!({}));
    }

    fn specialists() -> BTreeSet<String> {
        BTreeSet::from(["demo".to_owned()])
    }

    #[test]
    fn a_specialist_without_cases_and_without_a_ratchet_entry_fails() {
        let d = tmp("none");
        let f = check(&specialists(), &d, &[]);
        assert!(f.iter().any(|f| f.starts_with("demo needs")), "{f:?}");
        assert!(check(&specialists(), &d, &["demo"]).is_empty());
    }

    #[test]
    fn a_case_carrying_an_email_address_fails() {
        let d = tmp("email");
        put(&d, "demo/scoring.json", &json!({}));
        case(
            &d,
            "p",
            "positive",
            "Rückfrage an erika.beispiel@stadtwerk-beispiel.de",
        );
        case(&d, "n", "insufficient-evidence", "nichts");
        let f = check(&specialists(), &d, &[]);
        assert!(f.iter().any(|f| f.contains("an e-mail address")), "{f:?}");
    }

    #[test]
    fn clean_cases_pass_and_a_ratchet_entry_with_cases_fails() {
        let d = tmp("clean");
        put(&d, "demo/scoring.json", &json!({}));
        case(&d, "p", "positive", "Vorlauffrist wurde nicht eingehalten");
        case(&d, "n", "negative", "nichts");
        assert_eq!(check(&specialists(), &d, &[]), Vec::<String>::new());
        let f = check(&specialists(), &d, &["demo"]);
        assert!(
            f.iter().any(|f| f.contains("remove it from WITHOUT_CASES")),
            "{f:?}"
        );
    }

    #[test]
    fn an_iban_and_a_real_name_fail() {
        assert!(looks_like_iban("DE89370400440532013000"));
        assert!(!looks_like_iban("TariffDeviation"));
        assert!(!looks_like_iban("E_0622"));
        let mut f = Vec::new();
        personal_data(
            "x",
            &json!({ "nachname": "Schmidt", "vorname": "Erika" }),
            &mut f,
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }
}
