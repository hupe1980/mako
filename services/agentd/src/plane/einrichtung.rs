//! The Einrichtung's pre-fill, checked: an identifier the model proposes is
//! kept only where the operator's sentence contains it.
//!
//! `einrichtung-assistent` reads one sentence into the setup interview's
//! answers (`angaben`). The Rollen it names are its reading and the person
//! confirms each; an identifier — an MP-ID, a Bilanzkreis — is a fact, and a
//! fact the sentence does not contain is one the model made up. So
//! [`belegte_angaben`] drops every such value before the answer leaves agentd:
//! an `mp_id` or `primaer` becomes null, a list loses the entry. The interview
//! then shows that question unanswered, which is what it is.
//!
//! The comparison ignores whitespace and case only, so „9900 3570 00004" in
//! the sentence carries `9900357000004`; a changed digit carries nothing.

use serde_json::Value;

/// The input key the Desk sends the sentence under.
pub const SATZ: &str = "satz";

fn normal(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `angaben` with every identifier the sentence does not contain removed.
#[must_use]
pub fn belegte_angaben(angaben: &Value, satz: &str) -> Value {
    let satz = normal(satz);
    let steht_da = |v: &Value| {
        v.as_str()
            .map(normal)
            .is_some_and(|s| !s.is_empty() && satz.contains(&s))
    };
    let kennung = |v: &Value| if steht_da(v) { v.clone() } else { Value::Null };
    let liste = |v: &Value| -> Value {
        v.as_array().map_or(Value::Null, |a| {
            Value::Array(a.iter().filter(|x| steht_da(x)).cloned().collect())
        })
    };

    let rollen: Vec<Value> = angaben["rollen"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["rolle"].is_string())
        .map(|r| {
            serde_json::json!({
                "rolle": r["rolle"].clone(),
                "mp_id": kennung(&r["mp_id"]),
            })
        })
        .collect();
    let bilanzkreise: Vec<Value> = angaben["bilanzkreise"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["sparte"].is_string())
        .map(|b| {
            serde_json::json!({
                "sparte": b["sparte"].clone(),
                "standard": liste(&b["standard"]),
            })
        })
        .filter(|b| b["standard"].as_array().is_some_and(|a| !a.is_empty()))
        .collect();
    // `[]` is an answer („keine“), so an empty list stays; a list whose every
    // entry the sentence lacks is not that answer and becomes null.
    let verbunden = match angaben["verbundene_lieferanten"].as_array() {
        Some(a) if a.is_empty() => Value::Array(Vec::new()),
        Some(_) => match liste(&angaben["verbundene_lieferanten"]) {
            Value::Array(a) if a.is_empty() => Value::Null,
            v => v,
        },
        None => Value::Null,
    };
    serde_json::json!({
        "rollen": rollen,
        "primaer": kennung(&angaben["primaer"]),
        "bilanzkreise": bilanzkreise,
        "verbundene_lieferanten": verbunden,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An identifier the sentence carries survives, spaced or not; one it
    /// does not carry — a completed check digit, a code from nowhere — is
    /// dropped, and the Rolle stays for the person to answer.
    #[test]
    fn only_identifiers_the_sentence_contains_survive() {
        let satz = "Wir sind Lieferant Strom und Gas, Strom unter 9900 3570 00004, \
                    Bilanzkreis 11XBK-STD------Y.";
        let vom_modell = json!({
            "rollen": [
                { "rolle": "lf-strom", "mp_id": "9900357000004" },
                { "rolle": "lf-gas", "mp_id": "9800357000005" }
            ],
            "primaer": "9900357000004",
            "bilanzkreise": [
                { "sparte": "strom", "standard": ["11XBK-STD------Y"] },
                { "sparte": "gas", "standard": ["THE0BFH000000000"] }
            ],
            "verbundene_lieferanten": null
        });
        let belegt = belegte_angaben(&vom_modell, satz);
        assert_eq!(
            belegt,
            json!({
                "rollen": [
                    { "rolle": "lf-strom", "mp_id": "9900357000004" },
                    { "rolle": "lf-gas", "mp_id": null }
                ],
                "primaer": "9900357000004",
                "bilanzkreise": [{ "sparte": "strom", "standard": ["11XBK-STD------Y"] }],
                "verbundene_lieferanten": null
            })
        );
    }

    /// „Keine verbundenen Lieferanten“ is an answer and stays `[]`; a list of
    /// codes the sentence lacks is no answer.
    #[test]
    fn an_empty_list_is_an_answer_and_an_invented_one_is_not() {
        let leer = json!({ "rollen": [], "primaer": null, "bilanzkreise": [], "verbundene_lieferanten": [] });
        assert_eq!(
            belegte_angaben(&leer, "keine")["verbundene_lieferanten"],
            json!([])
        );
        let erfunden = json!({ "rollen": [], "primaer": null, "bilanzkreise": [], "verbundene_lieferanten": ["9900000000003"] });
        assert_eq!(
            belegte_angaben(&erfunden, "ein Satz ohne Code")["verbundene_lieferanten"],
            Value::Null
        );
    }

    /// A malformed answer is reduced to the shape, never passed through.
    #[test]
    fn a_malformed_answer_keeps_only_the_shape() {
        let v = belegte_angaben(&json!({ "frei": "text", "rollen": "lf-strom" }), "x");
        assert_eq!(
            v,
            json!({ "rollen": [], "primaer": null, "bilanzkreise": [], "verbundene_lieferanten": null })
        );
    }
}
