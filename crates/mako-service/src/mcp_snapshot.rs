//! Pin a service's MCP tool contract to a committed `mcp.snapshot.json`.
//!
//! The snapshot holds, per tool, its name, annotations, input schema and output
//! schema — the typing contract — and leaves out the description, which is
//! prose. Two readers depend on it: an agent learns from `outputSchema` what a
//! tool returns, and the Desk generates its TypeScript types from the file. A
//! field renamed in a tool's output type therefore has to change the snapshot
//! in the same commit, and the Desk's typecheck then fails wherever the old name
//! is read.
//!
//! A tool with no declared output schema is recorded with `outputSchema: null`,
//! so the snapshot also says which tools are not yet typed.
//!
//! Set `BLESS_MCP_SNAPSHOT=1` to rewrite the file instead of comparing.
//!
//! [`compare_json`] pins any other contract a service serves to the Desk the
//! same way — agentd's skill catalog is one — with the same variable and the
//! same line diff.
//!
//! ```rust,ignore
//! let tools: Vec<serde_json::Value> = Handler::tool_router()
//!     .list_all()
//!     .iter()
//!     .map(serde_json::to_value)
//!     .collect::<Result<_, _>>()?;
//! mako_service::mcp_snapshot::compare(&tools, &manifest_dir.join("mcp.snapshot.json"))?;
//! ```

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Why a tool list does not match its snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The snapshot file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The snapshot file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The committed snapshot is not what the service serves.
    #[error(
        "{path} does not match what the service serves; rerun with BLESS_MCP_SNAPSHOT=1 \
         and regenerate the Desk's types (`just desk-gen`) if the change is intended.\n\
         --- committed\n+++ served\n{diff}"
    )]
    Drift {
        /// The snapshot file.
        path: PathBuf,
        /// The lines that differ.
        diff: String,
    },
}

/// The snapshot text for a tool list: one object per tool, sorted by name,
/// holding the fields of the typing contract.
#[must_use]
pub fn render(tools: &[Value]) -> String {
    let mut entries: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let mut entry = Map::new();
            for key in ["name", "annotations", "inputSchema", "outputSchema"] {
                entry.insert(
                    key.to_owned(),
                    tool.get(key).cloned().unwrap_or(Value::Null),
                );
            }
            Value::Object(entry)
        })
        .collect();
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut text =
        serde_json::to_string_pretty(&Value::Array(entries)).unwrap_or_else(|_| String::from("[]"));
    text.push('\n');
    text
}

/// Compare a served tool list with the snapshot at `path`, or rewrite the
/// snapshot when `BLESS_MCP_SNAPSHOT` is set.
///
/// # Errors
///
/// [`SnapshotError::Drift`] when the file differs from [`render`]'s output,
/// [`SnapshotError::Io`] when it cannot be read or written.
#[must_use = "a snapshot comparison that is not checked proves nothing"]
pub fn compare(tools: &[Value], path: &Path) -> Result<(), SnapshotError> {
    compare_text(&render(tools), path)
}

/// Compare any served contract with the snapshot at `path`, pretty-printed as
/// it stands, or rewrite the snapshot when `BLESS_MCP_SNAPSHOT` is set.
///
/// # Errors
///
/// [`SnapshotError::Drift`] when the file differs, [`SnapshotError::Io`] when
/// it cannot be read or written.
#[must_use = "a snapshot comparison that is not checked proves nothing"]
pub fn compare_json(value: &Value, path: &Path) -> Result<(), SnapshotError> {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| String::from("null"));
    text.push('\n');
    compare_text(&text, path)
}

fn compare_text(served: &str, path: &Path) -> Result<(), SnapshotError> {
    let io = |source| SnapshotError::Io {
        path: path.to_owned(),
        source,
    };
    if std::env::var_os("BLESS_MCP_SNAPSHOT").is_some() {
        return std::fs::write(path, served).map_err(io);
    }
    let committed = std::fs::read_to_string(path).map_err(io)?;
    if committed == served {
        return Ok(());
    }
    let diff = diff_lines(&committed, served);
    Err(SnapshotError::Drift {
        path: path.to_owned(),
        diff,
    })
}

/// The lines present on one side only, in order — enough to name a renamed
/// field without a diff dependency.
fn diff_lines(committed: &str, served: &str) -> String {
    let old: std::collections::HashSet<&str> = committed.lines().collect();
    let new: std::collections::HashSet<&str> = served.lines().collect();
    let removed = committed
        .lines()
        .filter(|l| !new.contains(l))
        .map(|l| format!("-{l}"));
    let added = served
        .lines()
        .filter(|l| !old.contains(l))
        .map(|l| format!("+{l}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, output_field: &str) -> Value {
        json!({
            "name": name,
            "description": "prose that may change freely",
            "inputSchema": {"type": "object"},
            "outputSchema": {"type": "object", "properties": {output_field: {"type": "string"}}},
            "annotations": {"readOnlyHint": true},
        })
    }

    #[test]
    fn the_description_is_not_part_of_the_contract() {
        let mut a = tool("get_x", "id");
        let b = tool("get_x", "id");
        a["description"] = json!("reworded");
        assert_eq!(render(&[a]), render(&[b]));
    }

    #[test]
    fn a_renamed_output_field_is_drift() {
        let dir = std::env::temp_dir().join(format!("mcp-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mcp.snapshot.json");
        std::fs::write(&path, render(&[tool("get_x", "id")])).expect("write");

        assert!(compare(&[tool("get_x", "id")], &path).is_ok());
        let err = compare(&[tool("get_x", "identifier")], &path).expect_err("drift");
        let SnapshotError::Drift { diff, .. } = err else {
            panic!("expected drift, got {err}");
        };
        assert!(diff.contains('-') && diff.contains("\"id\""), "{diff}");
        assert!(diff.contains("\"identifier\""), "{diff}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_changed_value_is_drift_with_its_line() {
        let dir = std::env::temp_dir().join(format!("json-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("catalog.snapshot.json");
        let pinned = json!({"liest": ["makod/get_process"]});
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&pinned).expect("json")),
        )
        .expect("write");

        assert!(compare_json(&pinned, &path).is_ok());
        let err = compare_json(
            &json!({"liest": ["makod/get_process", "makod/post_command"]}),
            &path,
        )
        .expect_err("drift");
        assert!(
            err.to_string().contains("+    \"makod/post_command\""),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_untyped_tool_is_recorded_as_untyped() {
        let text = render(&[json!({"name": "legacy", "inputSchema": {}})]);
        assert!(text.contains("\"outputSchema\": null"), "{text}");
    }

    #[test]
    fn tools_are_ordered_by_name() {
        let text = render(&[tool("b", "x"), tool("a", "x")]);
        assert!(text.find("\"a\"") < text.find("\"b\""), "{text}");
    }
}
