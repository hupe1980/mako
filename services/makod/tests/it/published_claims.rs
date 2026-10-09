//! Guard: what the published pages tell an integrator about `makod` holds.
//!
//! Two claims an integrator builds against and that fail silently on our side
//! when the page is wrong:
//!
//! - **Command examples.** Every `curl` to `POST /api/v1/commands` printed in
//!   the site or a README is pasted as is. Its body must deserialise into
//!   [`ErpCommand`] — which denies unknown fields, so a `malo_id` beside
//!   `payload` instead of inside it is a 422 — name a registered command, and
//!   carry the `Idempotency-Key` header every command needs.
//! - **The Redispatch acknowledgement window.** Every statement of the
//!   `AcknowledgementDocument` Frist agrees with
//!   [`mako_redispatch::fristen::ACK_FRIST`]. A page saying six hours sizes an
//!   acknowledgement path 120 times too slow; the figure is widely quoted, so a
//!   page may name it — but only to say it is not the obligation.

use std::path::{Path, PathBuf};

use makod::commands_api::{CommandError, ErpCommand, validate_command};

/// The repository root, from this test's own location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above services/makod")
}

/// Every Markdown page an integrator reads: the site and every README.
fn published_pages() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    walk(&root.join("site/content"), &mut paths);
    paths.push(root.join("README.md"));
    for group in ["crates", "services"] {
        for e in std::fs::read_dir(root.join(group))
            .into_iter()
            .flatten()
            .flatten()
        {
            let readme = e.path().join("README.md");
            if readme.is_file() {
                paths.push(readme);
            }
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let md = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("{} is not readable: {e}", p.display()));
            let name = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
            (name, md)
        })
        .collect()
}

// ── Command examples ─────────────────────────────────────────────────────────

/// Split a shell command line into words the way `sh` does for the cases the
/// pages print: `'…'` literal, `"…"` with `\"`, `\\`, `\$` escapes, and a
/// backslash-newline as whitespace. Stops at an unquoted newline.
fn shell_words(src: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\' | '$')) => {
                            cur.extend(chars.next());
                        }
                        _ => cur.push(q),
                    }
                }
            }
            '\\' if chars.peek() == Some(&'\n') => {
                chars.next();
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\n' => break,
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// One printed `curl` call: where it is, its URL, headers and body.
struct Curl {
    at: String,
    url: String,
    headers: Vec<String>,
    body: Option<String>,
}

/// Every `curl` call in the `bash`/`sh` blocks of a page.
fn curl_calls(name: &str, md: &str) -> Vec<Curl> {
    let mut calls = Vec::new();
    let mut in_shell = false;
    let mut offset = 0;
    for (n, line) in md.lines().enumerate() {
        let start = offset;
        offset += line.len() + 1;
        let t = line.trim_start();
        if t.starts_with("```") {
            in_shell = !in_shell && matches!(t.trim_start_matches('`').trim(), "bash" | "sh");
            continue;
        }
        if !in_shell || !t.starts_with("curl ") {
            continue;
        }
        let words = shell_words(&md[start..]);
        let mut call = Curl {
            at: format!("{name}:{}", n + 1),
            url: String::new(),
            headers: Vec::new(),
            body: None,
        };
        let mut it = words.into_iter().skip(1);
        while let Some(w) = it.next() {
            match w.as_str() {
                "-H" | "--header" => call.headers.extend(it.next()),
                "-d" | "--data" | "--data-raw" => call.body = it.next(),
                "-X" | "--request" => {
                    it.next();
                }
                w if w.contains("://") => call.url = w.to_owned(),
                _ => {}
            }
        }
        calls.push(call);
    }
    calls
}

/// What is wrong with one printed command call, if anything.
fn command_example_defects(call: &Curl) -> Vec<String> {
    let mut defects = Vec::new();
    if !call
        .headers
        .iter()
        .any(|h| h.to_ascii_lowercase().starts_with("idempotency-key:"))
    {
        defects.push("no `Idempotency-Key` header — makod answers 422".to_owned());
    }
    let Some(body) = &call.body else {
        defects.push("no body".to_owned());
        return defects;
    };
    match serde_json::from_str::<ErpCommand>(body) {
        Err(e) => defects.push(format!("the body is not an ErpCommand — {e}")),
        Ok(cmd) => {
            const EVERY_ROLE: &[&str] = &[
                "LF", "NB", "MSB", "LFG", "GNB", "NMSB", "BKV", "ÜNB", "UNB", "BIKO", "MGV", "ESA",
                "EIV",
            ];
            let configured: Vec<String> = EVERY_ROLE.iter().map(|r| (*r).to_owned()).collect();
            match validate_command(&cmd.command, cmd.marktrolle.as_deref(), &configured) {
                Ok(_) => {}
                Err(CommandError::UnknownCommand) => {
                    defects.push(format!("`{}` is not a registered command", cmd.command));
                }
                Err(e) => defects.push(format!("`{}` is refused: {e}", cmd.command)),
            }
        }
    }
    defects
}

#[test]
fn every_printed_command_example_is_accepted() {
    let calls: Vec<Curl> = published_pages()
        .iter()
        .flat_map(|(name, md)| curl_calls(name, md))
        .filter(|c| c.url.ends_with("/api/v1/commands"))
        .collect();
    assert!(
        calls
            .iter()
            .any(|c| c.at.starts_with("site/content/docs/services/makod.md")),
        "the makod guide prints command examples; the extraction must find them"
    );
    let wrong: Vec<String> = calls
        .iter()
        .flat_map(|c| {
            command_example_defects(c)
                .into_iter()
                .map(move |d| format!("{}: {d}", c.at))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "printed `POST /api/v1/commands` examples makod refuses:\n{}",
        wrong.join("\n")
    );
}

/// The check has to see the two defects it exists for.
#[test]
fn a_planted_command_example_with_a_top_level_malo_id_and_no_key_is_refused() {
    let md = "```bash\ncurl -X POST http://localhost:8080/api/v1/commands \\\n  \
              -H \"Content-Type: application/json\" \\\n  -d '{\n    \
              \"command\": \"gpke.sperrung.bestaetigen\",\n    \"marktrolle\": \"NB\",\n    \
              \"malo_id\": \"51238696012\",\n    \"payload\": {}\n  }'\n```\n";
    let calls = curl_calls("fixture.md", md);
    assert_eq!(calls.len(), 1);
    let defects = command_example_defects(&calls[0]);
    assert!(
        defects.iter().any(|d| d.contains("Idempotency-Key")),
        "{defects:?}"
    );
    assert!(defects.iter().any(|d| d.contains("malo_id")), "{defects:?}");
}

// ── The Redispatch acknowledgement window ────────────────────────────────────

/// Spelled-out numbers the pages use for a duration.
fn number(word: &str) -> Option<i64> {
    Some(match word {
        "one" | "eine" | "einer" | "ein" => 1,
        "two" | "zwei" => 2,
        "three" | "drei" => 3,
        "five" | "fünf" => 5,
        "six" | "sechs" => 6,
        "ten" | "zehn" => 10,
        "twelve" | "zwölf" => 12,
        "fifteen" | "fünfzehn" => 15,
        "thirty" | "dreißig" => 30,
        w => return w.parse().ok(),
    })
}

fn unit(word: &str) -> Option<time::Duration> {
    Some(match word {
        "min" | "minute" | "minutes" | "minuten" => time::Duration::MINUTE,
        "h" | "hour" | "hours" | "stunde" | "stunden" => time::Duration::HOUR,
        "s" | "sec" | "second" | "seconds" | "sekunde" | "sekunden" => time::Duration::SECOND,
        _ => return None,
    })
}

/// The units a page states one claim in: a table row, or a sentence of a
/// paragraph (blockquote markers dropped).
fn claim_units(md: &str) -> Vec<String> {
    let mut units = Vec::new();
    for para in md.split("\n\n") {
        let lines: Vec<&str> = para
            .lines()
            .map(|l| l.trim_start().trim_start_matches('>').trim())
            .collect();
        if lines.iter().all(|l| l.starts_with('|')) {
            units.extend(lines.iter().map(|l| (*l).to_owned()));
            continue;
        }
        let joined = lines.join(" ");
        units.extend(joined.split(". ").map(str::to_owned));
    }
    units
}

/// The acknowledgement windows `claim` states that are not `ACK_FRIST` and are
/// not said to be wrong.
fn contradicting_windows(claim: &str, ack: time::Duration) -> Vec<time::Duration> {
    let lower = claim.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let redispatch = words.contains(&"redispatch");
    let mentions: Vec<usize> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            **w == "acknowledgementdocument" || (redispatch && **w == "acknowledgement")
        })
        .map(|(i, _)| i)
        .collect();
    if mentions.is_empty() {
        return Vec::new();
    }
    let durations: Vec<(usize, time::Duration)> = words
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| Some((i, unit(w[1])? * i32::try_from(number(w[0])?).ok()?)))
        .collect();
    let negated = words
        .iter()
        .any(|w| matches!(*w, "not" | "never" | "nicht" | "nie" | "kein" | "keine"))
        || lower.contains("widely quoted");
    mentions
        .iter()
        .filter_map(|m| {
            durations
                .iter()
                .min_by_key(|(i, _)| i.abs_diff(*m))
                .map(|(_, d)| *d)
        })
        .filter(|d| *d != ack && !negated)
        .collect()
}

fn ack_window_defects(name: &str, md: &str) -> Vec<String> {
    let ack = mako_redispatch::fristen::ACK_FRIST;
    claim_units(md)
        .iter()
        .flat_map(|u| {
            contradicting_windows(u, ack)
                .into_iter()
                .map(move |d| format!("{name}: states {d} — „{}“", u.trim()))
        })
        .collect()
}

#[test]
fn every_published_acknowledgement_window_is_ack_frist() {
    let wrong: Vec<String> = published_pages()
        .iter()
        .flat_map(|(name, md)| ack_window_defects(name, md))
        .collect();
    assert!(
        wrong.is_empty(),
        "these pages state a Redispatch AcknowledgementDocument window other than \
         mako_redispatch::fristen::ACK_FRIST ({}) — AcknowledgementDocument FB 1.0g, \
         „Fristen zur Übermittlung der AcknowledgementDocument-Nachricht“:\n{}",
        mako_redispatch::fristen::ACK_FRIST,
        wrong.join("\n")
    );
}

#[test]
fn a_planted_six_hour_acknowledgement_window_is_refused() {
    let md = "The Redispatch `AcknowledgementDocument` is due within **6 hours** of receipt.\n\n\
              | `AcknowledgementDocument` | 6 Stunden | FB 1.0g |\n";
    assert_eq!(ack_window_defects("fixture.md", md).len(), 2);
}

#[test]
fn the_three_minute_window_and_its_refutation_pass() {
    let md = "The `AcknowledgementDocument` is due „unverzüglich, jedoch spätestens \
              **3 Minuten**“. The widely quoted 6 hours was never that obligation.\n\n\
              > **The acknowledgement is three minutes, not six hours.** Redispatch.\n\n\
              | `AcknowledgementDocument` | 3 min | sourced. Never six hours |\n";
    assert!(ack_window_defects("fixture.md", md).is_empty());
}
