//! Quoting a rule by its address: `quote_source` and `find_source`.
//!
//! A model states a rule only as a quote. It names the document, its edition
//! and a chapter or paragraph — `{doc: "APERAK AHB", edition: "1.1", at:
//! "2.4.1"}`, `{doc: "EnWG", at: "§ 41f Abs. 5"}` — and gets the passage
//! verbatim with its page and the document's SHA-256. An address the index
//! does not hold is refused, never answered with the nearest passage: „A02"
//! means four refusals in four Entscheidungsbäume and five format releases
//! bind at once, so the nearest passage is the plausible wrong one.
//! `find_source` takes words and answers addresses only, never text a model
//! could paraphrase instead of quoting.
//!
//! The index is `cargo xtask index-sources`' file, built on the operator's
//! machine from their own `regulatories/` mirror and never shipped. Without
//! one both tools refuse every call and say why; a run then has no quote, and
//! Belegprüfung marks every sentence naming a rule „nicht belegt"
//! ([`super::beleg`]).
//!
//! Both tools are compiled into agentd and read a file — they reach no
//! service and change nothing, which [`WERKZEUGE`] records for
//! `check-tool-grants`.

use std::path::Path;
use std::sync::Arc;

use agentplane::tools::{Destination, ToolClient, ToolError, ToolId};
use serde::Deserialize;
use serde_json::{Value, json};

/// The server name a grant names these tools by: `tool://agentd/quote_source`.
pub const SERVER: &str = "agentd";

/// One tool this module serves.
#[derive(Debug, Clone, Copy)]
pub struct Werkzeug {
    pub name: &'static str,
    /// Always true: the tools read the index file and nothing else.
    pub read_only: bool,
}

/// The tools served under [`SERVER`]. `check-tool-grants` reads this list as
/// the server's own declaration, the way it reads a service's
/// `read_only_hint`.
pub const WERKZEUGE: &[Werkzeug] = &[
    Werkzeug {
        name: "quote_source",
        read_only: true,
    },
    Werkzeug {
        name: "find_source",
        read_only: true,
    },
];

/// How many addresses `find_source` answers at most.
const TREFFER: usize = 10;

/// The passage index `cargo xtask index-sources` writes.
#[derive(Debug, Deserialize)]
pub struct Quellenindex {
    dokumente: Vec<Dokument>,
}

#[derive(Debug, Deserialize)]
struct Dokument {
    doc: String,
    edition: String,
    datei: String,
    sha256: String,
    stellen: Vec<Stelle>,
}

#[derive(Debug, Deserialize)]
struct Stelle {
    at: String,
    #[serde(default)]
    seite: Option<u32>,
    #[serde(default)]
    titel: Option<String>,
    text: String,
}

impl Quellenindex {
    /// Read the index file.
    ///
    /// # Errors
    ///
    /// The file is absent, unreadable, not the index's shape, or holds no
    /// passage — an empty index would refuse every address while looking
    /// built.
    pub fn laden(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read(path).map_err(|e| {
            format!(
                "Quellenindex {} nicht lesbar ({e}) — `cargo xtask index-sources` baut ihn",
                path.display()
            )
        })?;
        Self::from_slice(&raw)
    }

    /// Parse an index.
    ///
    /// # Errors
    ///
    /// Not the index's shape, or no passage in it.
    pub fn from_slice(raw: &[u8]) -> Result<Self, String> {
        let index: Self =
            serde_json::from_slice(raw).map_err(|e| format!("Quellenindex unlesbar: {e}"))?;
        if index.dokumente.iter().all(|d| d.stellen.is_empty()) {
            return Err("Quellenindex enthält keine Stelle".to_owned());
        }
        Ok(index)
    }

    /// The passage at an address, verbatim.
    ///
    /// `edition` may be left out only where the index holds the document in
    /// one edition; with several — an AHB in the edition binding now and the
    /// one binding before — the address is incomplete and refused.
    ///
    /// # Errors
    ///
    /// The address names no passage of the index, or is ambiguous.
    pub fn zitat(&self, doc: &str, edition: Option<&str>, at: &str) -> Result<Value, String> {
        let at = normal_at(at);
        let mut editions: Vec<&Dokument> = self
            .dokumente
            .iter()
            .filter(|d| d.doc.eq_ignore_ascii_case(doc.trim()))
            .collect();
        if editions.is_empty() {
            return Err(format!("„{doc}“ ist nicht im Quellenindex"));
        }
        if let Some(e) = edition.map(str::trim).filter(|e| !e.is_empty()) {
            editions.retain(|d| d.edition == e);
            if editions.is_empty() {
                return Err(format!("„{doc}“ liegt nicht in der Ausgabe „{e}“ vor"));
            }
        }
        let [d] = editions.as_slice() else {
            let alle: Vec<&str> = editions.iter().map(|d| d.edition.as_str()).collect();
            return Err(format!(
                "„{doc}“ liegt in mehreren Ausgaben vor ({}) — nenne `edition`",
                alle.join(", ")
            ));
        };
        let Some(s) = d.stellen.iter().find(|s| s.at == at) else {
            return Err(format!(
                "„{doc}“ {} hat keine Stelle „{at}“ — keine nächstgelegene wird angeboten; \
                 `find_source` nennt die Adressen",
                d.edition
            ));
        };
        Ok(json!({
            "doc": d.doc,
            "edition": d.edition,
            "at": s.at,
            "seite": s.seite,
            "titel": s.titel,
            "text": s.text,
            "datei": d.datei,
            "sha256": d.sha256,
        }))
    }

    /// The addresses whose passages carry the most of `worte`, best first —
    /// addresses only, no text.
    #[must_use]
    pub fn finden(&self, worte: &str, doc: Option<&str>) -> Vec<Value> {
        let worte: Vec<String> = worte
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 3)
            .map(str::to_lowercase)
            .collect();
        if worte.is_empty() {
            return Vec::new();
        }
        let mut treffer: Vec<(usize, &Dokument, &Stelle)> = self
            .dokumente
            .iter()
            .filter(|d| doc.is_none_or(|x| d.doc.eq_ignore_ascii_case(x.trim())))
            .flat_map(|d| d.stellen.iter().map(move |s| (d, s)))
            .filter_map(|(d, s)| {
                let text = s.text.to_lowercase();
                let n = worte.iter().filter(|w| text.contains(w.as_str())).count();
                (n > 0).then_some((n, d, s))
            })
            .collect();
        // Most words first; among equals the shorter passage, which is the
        // more specific address (an Absatz before its whole §).
        treffer.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.text.len().cmp(&b.2.text.len())));
        treffer
            .into_iter()
            .take(TREFFER)
            .map(|(_, d, s)| {
                json!({ "doc": d.doc, "edition": d.edition, "at": s.at, "seite": s.seite })
            })
            .collect()
    }
}

/// An address in the index's own spelling: `§41f Abs.5` and `§ 41f Absatz 5`
/// are `§ 41f Abs. 5`; a chapter is its number, so `Kap. 2.4.1` and a
/// dotted `§ 2.4.1` written for it are `2.4.1`.
#[must_use]
pub fn normal_at(at: &str) -> String {
    let t = at.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim();
    for prefix in ["Kapitel ", "Kap. ", "Kap.", "§ ", "§"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.contains('.') && rest.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return rest.to_owned();
            }
        }
    }
    let Some(rest) = t.strip_prefix('§') else {
        return t.to_owned();
    };
    let rest = rest.trim();
    let (num, tail) = rest.split_once(' ').unwrap_or((rest, ""));
    let tail = tail.trim();
    let abs = tail
        .strip_prefix("Absatz")
        .or_else(|| tail.strip_prefix("Abs."))
        .map(str::trim);
    match abs {
        Some(a) if !a.is_empty() => format!("§ {num} Abs. {a}"),
        _ if tail.is_empty() => format!("§ {num}"),
        _ => format!("§ {num} {tail}"),
    }
}

/// The in-process tool server: the index, or why there is none.
#[derive(Debug, Clone)]
pub struct Quellen {
    index: Result<Arc<Quellenindex>, String>,
}

impl Quellen {
    /// Serve `index`.
    #[must_use]
    pub fn new(index: Quellenindex) -> Self {
        Self {
            index: Ok(Arc::new(index)),
        }
    }

    /// Serve nothing: every call is refused with `warum`.
    #[must_use]
    pub fn ohne(warum: impl Into<String>) -> Self {
        Self {
            index: Err(warum.into()),
        }
    }

    /// The index at `path`, or a server that refuses every call saying why.
    #[must_use]
    pub fn aus(path: Option<&Path>) -> Self {
        match path {
            None => Self::ohne(
                "kein Quellenindex konfiguriert ([quellen] index) — `cargo xtask \
                 index-sources` baut ihn aus dem regulatories/-Spiegel",
            ),
            Some(p) => Quellenindex::laden(p).map_or_else(Self::ohne, Self::new),
        }
    }

    /// Whether an index is loaded.
    #[must_use = "an index that failed to load is to be reported"]
    pub fn bereit(&self) -> Result<(), &str> {
        self.index.as_ref().map(|_| ()).map_err(String::as_str)
    }
}

fn arg<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments[key].as_str().filter(|s| !s.trim().is_empty())
}

#[async_trait::async_trait]
impl ToolClient for Quellen {
    /// Reads a local file: no connection is opened.
    fn destination(&self, _tool: &ToolId) -> Destination {
        Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        let refused = |detail: String| ToolError::Refused {
            tool: tool.clone(),
            detail,
        };
        let index = self.index.as_ref().map_err(|e| refused(e.clone()))?;
        match tool.tool.as_str() {
            "quote_source" => {
                let (Some(doc), Some(at)) = (arg(arguments, "doc"), arg(arguments, "at")) else {
                    return Err(refused("`doc` und `at` sind Pflicht".to_owned()));
                };
                index
                    .zitat(doc, arg(arguments, "edition"), at)
                    .map_err(refused)
            }
            "find_source" => {
                let Some(worte) = arg(arguments, "worte") else {
                    return Err(refused("`worte` ist Pflicht".to_owned()));
                };
                Ok(json!({ "adressen": index.finden(worte, arg(arguments, "doc")) }))
            }
            other => Err(refused(format!("agentd serves no tool `{other}`"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn index() -> Quellenindex {
        Quellenindex::from_slice(
            json!({ "dokumente": [
                { "doc": "APERAK AHB", "edition": "1.0", "datei": "bdew-mako/APERAK_AHB_1.0.pdf",
                  "sha256": "b42a", "stellen": [
                    { "at": "2.4.1", "seite": 22, "titel": "Fristen", "text": "Fassung 1.0: binnen 45 Minuten." } ] },
                { "doc": "APERAK AHB", "edition": "1.1", "datei": "bdew-mako/APERAK_AHB_1.1.pdf",
                  "sha256": "6c1a", "stellen": [
                    { "at": "2.4.1", "seite": 24, "titel": "Fristen", "text": "Fassung 1.1: binnen 45 Minuten." } ] },
                { "doc": "EnWG", "edition": "Stand 2026-10-22", "datei": "gesetze/enwg_2005.xml",
                  "sha256": "03b3", "stellen": [
                    { "at": "§ 41f Abs. 1", "text": "(1) Bei der Nichterfüllung einer Zahlungsverpflichtung …" },
                    { "at": "§ 41f Abs. 5", "text": "(5) Der Beginn der Unterbrechung der Energieversorgung ist dem Haushaltskunden acht Werktage im Voraus durch briefliche Mitteilung anzukündigen." } ] }
            ]})
            .to_string()
            .as_bytes(),
        )
        .expect("an index")
    }

    #[test]
    fn an_address_the_index_lacks_is_refused_not_answered_nearby() {
        let i = index();
        let err = i
            .zitat("EnWG", None, "§ 41f Abs. 4")
            .expect_err("Abs. 4 is not indexed");
        assert!(err.contains("keine nächstgelegene"), "{err}");
        assert!(i.zitat("StromGVV", None, "§ 19").is_err());
        assert!(i.zitat("APERAK AHB", Some("1.1"), "2.4.2").is_err());
    }

    #[test]
    fn aperak_ahb_1_0_and_1_1_are_separate_addresses() {
        let i = index();
        let z10 = i.zitat("APERAK AHB", Some("1.0"), "2.4.1").expect("1.0");
        let z11 = i
            .zitat("APERAK AHB", Some("1.1"), "Kap. 2.4.1")
            .expect("1.1");
        assert_eq!(z10["text"], "Fassung 1.0: binnen 45 Minuten.");
        assert_eq!(z11["text"], "Fassung 1.1: binnen 45 Minuten.");
        assert_eq!(
            (z10["sha256"].as_str(), z11["seite"].as_u64()),
            (Some("b42a"), Some(24))
        );
        let err = i
            .zitat("APERAK AHB", None, "2.4.1")
            .expect_err("two editions, none named");
        assert!(err.contains("1.0, 1.1"), "{err}");
    }

    #[test]
    fn a_statute_address_is_read_in_any_spelling() {
        let i = index();
        for at in [
            "§ 41f Abs. 5",
            "§41f Abs.5",
            "§ 41f Absatz 5",
            "  §  41f   Abs. 5 ",
        ] {
            let z = i.zitat("EnWG", None, at).expect(at);
            assert_eq!(z["at"], "§ 41f Abs. 5");
        }
    }

    #[test]
    fn find_source_answers_addresses_and_no_text() {
        let found = index().finden("Unterbrechung acht Werktage", Some("EnWG"));
        assert_eq!(found[0]["at"], "§ 41f Abs. 5");
        assert!(found.iter().all(|a| a.get("text").is_none()), "{found:?}");
    }

    #[tokio::test]
    async fn without_an_index_every_call_is_refused_with_the_reason() {
        let q = Quellen::ohne("kein Index");
        let tool = ToolId::new(SERVER, "quote_source");
        let err = q
            .call(&tool, &json!({ "doc": "EnWG", "at": "§ 41f" }), None)
            .await
            .expect_err("refused");
        assert!(matches!(err, ToolError::Refused { ref detail, .. } if detail == "kein Index"));
    }

    /// Against the operator's own index, where one is built: § 41f Abs. 5
    /// EnWG verbatim, and the two APERAK AHB editions apart. Without the
    /// mirror there is nothing to hold, and the test says so rather than
    /// passing on an empty index.
    #[test]
    fn the_built_index_quotes_its_addresses_verbatim() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../regulatories/quellen-index.json");
        let Ok(index) = Quellenindex::laden(&path) else {
            eprintln!(
                "SKIPPED the_built_index_quotes_its_addresses_verbatim: no {} — run `cargo \
                 xtask index-sources --download` on a machine with the regulatories/ mirror",
                path.display()
            );
            return;
        };
        let z = index
            .zitat("EnWG", None, "§ 41f Abs. 5")
            .expect("§ 41f Abs. 5 EnWG");
        assert!(
            z["text"]
                .as_str()
                .is_some_and(|t| t
                    .contains("acht Werktage im Voraus durch briefliche Mitteilung anzukündigen")),
            "{z}"
        );
        let a10 = index
            .zitat("APERAK AHB", Some("1.0"), "2.4.1")
            .expect("the FV2025-10-01 edition");
        let a11 = index
            .zitat("APERAK AHB", Some("1.1"), "2.4.1")
            .expect("APERAK AHB 1.1");
        assert_ne!(a10["sha256"], a11["sha256"]);
        assert!(index.zitat("EnWG", None, "§ 41f Abs. 99").is_err());
    }

    #[test]
    fn an_empty_index_is_no_index() {
        assert!(Quellenindex::from_slice(br#"{"dokumente": []}"#).is_err());
    }
}
