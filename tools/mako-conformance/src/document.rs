//! A committed OpenAPI 3.1 document: its operations, and a body held to the
//! schema it declares.

use serde_json::{Value, json};

/// One documented operation.
#[derive(Debug, Clone)]
pub struct Operation {
    /// Upper-case method.
    pub method: String,
    /// The path template (`/api/v1/vertraege/{id}`).
    pub path: String,
    spec: Value,
}

impl Operation {
    /// `GET /api/v1/…`.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    /// Whether the operation asks for a token — it documents a `401`.
    #[must_use]
    pub fn protected(&self) -> bool {
        self.spec["responses"].get("401").is_some()
    }

    /// Whether the path carries a parameter.
    #[must_use]
    pub fn has_path_params(&self) -> bool {
        self.path.contains('{')
    }

    /// Whether a query parameter is required.
    #[must_use]
    pub fn requires_query(&self) -> bool {
        self.spec["parameters"].as_array().is_some_and(|ps| {
            ps.iter()
                .any(|p| p["in"] == "query" && p["required"].as_bool().unwrap_or(false))
        })
    }

    /// A success response with a JSON schema permits a body probe for this operation.
    #[must_use]
    pub fn declares_success_json(&self) -> bool {
        (200..300).any(|status| {
            self.response(status).is_some_and(|response| {
                !response["content"]["application/json"]["schema"].is_null()
            })
        })
    }

    /// The response entry for `status`, by exact code, `NXX` range or `default`.
    #[must_use]
    pub fn response(&self, status: u16) -> Option<&Value> {
        let r = &self.spec["responses"];
        r.get(status.to_string())
            .or_else(|| r.get(format!("{}XX", status / 100)))
            .or_else(|| r.get("default"))
    }
}

/// A parsed document.
#[derive(Debug, Clone)]
pub struct Document {
    raw: Value,
}

impl Document {
    /// # Errors
    ///
    /// When `text` is not JSON.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let raw: Value = serde_json::from_str(text)?;
        anyhow::ensure!(
            raw["openapi"]
                .as_str()
                .is_some_and(|v| v.starts_with("3.1.")),
            "expected an OpenAPI 3.1 document"
        );
        let paths = raw["paths"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("missing OpenAPI paths"))?;
        for (template, item) in paths {
            anyhow::ensure!(
                template.starts_with('/') && !template.starts_with("//") && item.is_object(),
                "invalid OpenAPI path item"
            );
        }
        let document = Self { raw };
        for op in document.operations() {
            let responses = op.spec["responses"]
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("missing OpenAPI responses"))?;
            if op.method == "GET" && !op.has_path_params() && !op.requires_query() {
                for response in responses.values() {
                    let schema = &response["content"]["application/json"]["schema"];
                    if !schema.is_null() {
                        jsonschema::draft202012::new(&document.rooted(schema.clone()))
                            .map_err(|_| anyhow::anyhow!("invalid reference response schema"))?;
                    }
                }
            }
        }
        Ok(document)
    }

    /// Every operation, sorted by path and method.
    #[must_use]
    pub fn operations(&self) -> Vec<Operation> {
        let mut out = Vec::new();
        if let Some(paths) = self.raw["paths"].as_object() {
            for (path, item) in paths {
                let Some(item) = item.as_object() else {
                    continue;
                };
                for (method, spec) in item {
                    if matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                        out.push(Operation {
                            method: method.to_uppercase(),
                            path: path.clone(),
                            spec: spec.clone(),
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
        out
    }

    /// The violations of `body` against the JSON schema `response` declares,
    /// `$ref`s resolved within this document. `None` when the response
    /// declares no JSON body.
    #[must_use]
    pub fn violations(&self, response: &Value, body: &Value) -> Option<Vec<String>> {
        let schema = response["content"]["application/json"]["schema"].clone();
        if schema.is_null() {
            return None;
        }
        Some(validate(&self.rooted(schema), body))
    }

    /// `schema` with this document's `components` beside it, so a
    /// `#/components/schemas/…` reference resolves.
    fn rooted(&self, schema: Value) -> Value {
        let mut root = match schema {
            Value::Object(m) => Value::Object(m),
            other => json!({ "allOf": [other] }),
        };
        root["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
        root["components"] = self.raw["components"].clone();
        root
    }
}

/// The violations of `instance` against the 2020-12 schema `schema`.
#[must_use]
pub fn validate(schema: &Value, instance: &Value) -> Vec<String> {
    match jsonschema::draft202012::new(schema) {
        Ok(v) => v
            .iter_errors(instance)
            .map(|e| {
                let at = e.instance_path().to_string();
                if at.is_empty() {
                    e.to_string()
                } else {
                    format!("{e} at {at}")
                }
            })
            .collect(),
        Err(e) => vec![format!("the published schema does not compile: {e}")],
    }
}

/// A body is mako's `ApiError`: a JSON object naming the error.
///
/// # Errors
///
/// What the body lacks.
pub fn api_error(body: &Value) -> Result<(), String> {
    if body.get("error").and_then(Value::as_str).is_some() {
        Ok(())
    } else {
        Err(format!(
            "the body is not an ApiError (a JSON object with `error`): {}",
            truncate(&body.to_string())
        ))
    }
}

/// A body cut to fit one report line.
#[must_use]
pub fn truncate(s: &str) -> String {
    const MAX: usize = 240;
    if s.chars().count() <= MAX {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::parse(
            &json!({
                "openapi": "3.1.0",
                "paths": {
                    "/a": { "get": { "responses": {
                        "200": { "content": { "application/json": {
                            "schema": { "$ref": "#/components/schemas/A" } } } },
                        "401": {}
                    } } }
                },
                "components": { "schemas": { "A": {
                    "type": "object", "required": ["n"],
                    "properties": { "n": { "type": "integer" } }
                } } }
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn a_reference_resolves_and_a_wrong_body_is_named() {
        let d = doc();
        let op = &d.operations()[0];
        assert!(op.protected());
        let r = op.response(200).unwrap();
        assert_eq!(d.violations(r, &json!({ "n": 1 })), Some(vec![]));
        let v = d.violations(r, &json!({ "n": "x" })).unwrap();
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("/n"), "{v:?}");
        assert!(op.response(500).is_none(), "an undocumented status");
    }
}
