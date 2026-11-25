//! Diagnose individual probes or assess the full replacement contract.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, ensure};
use mako_conformance::report::{Runner, valid_target_label};
use mako_conformance::suite::Target;
use tempfile::NamedTempFile;

const USAGE: &str = "\
mako-conformance <service> <test-url> --token <bearer> [options]

  --acceptance --target-build <label> --report <file>
                               assess the full contract and write JSON evidence
  --refused-token <bearer>     principal refused on the REST and MCP refusal probes
  --inbound-secret <secret>    secret for the inbound event receiver
  --listen <addr>              local delivery listener (default 127.0.0.1:9198)
  --callback <url>             URL the test target uses to reach the listener
  --wait <seconds>             delivery observation window (default 30)
  --only rest|events|mcp       select a runner (partial acceptance is incomplete)
  -h, --help                   print this help

Example (isolated seeded test deployment):
  mako-conformance vertragd http://127.0.0.1:9780 --token \"$TEST_TOKEN\" \\
    --acceptance --target-build demo-1 --report evidence.json

Acceptance exits: 0 passed, 1 failed, 2 usage or output error, 3 incomplete.
The diagnostic mode exits 1 for broken probes. Seeds write test records.";

struct Acceptance {
    label: String,
    path: PathBuf,
}

struct Options {
    service: String,
    target: Target,
    only: Option<Runner>,
    acceptance: Option<Acceptance>,
}

#[must_use = "invalid arguments must produce a usage error"]
fn parse(args: &[String]) -> anyhow::Result<Options> {
    let mut positional = Vec::new();
    let mut target = Target {
        base: String::new(),
        token: String::new(),
        refused_token: None,
        inbound_secret: None,
        listen: "127.0.0.1:9198".to_owned(),
        callback: None,
        wait: Duration::from_secs(30),
    };
    let mut only = None;
    let mut acceptance = false;
    let mut label = None;
    let mut path = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .filter(|v| !v.starts_with("--") && !v.is_empty())
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--acceptance" => acceptance = true,
            "--target-build" => label = Some(value()?),
            "--report" => path = Some(PathBuf::from(value()?)),
            "--token" => target.token = value()?,
            "--refused-token" => target.refused_token = Some(value()?),
            "--inbound-secret" => target.inbound_secret = Some(value()?),
            "--listen" => target.listen = value()?,
            "--callback" => target.callback = Some(value()?),
            "--wait" => {
                target.wait =
                    Duration::from_secs(value()?.parse().context("--wait takes seconds")?);
                ensure!(
                    std::time::Instant::now().checked_add(target.wait).is_some(),
                    "--wait is too large"
                );
            }
            "--only" => {
                only = Some(Runner::parse(&value()?).context("--only takes rest, events or mcp")?)
            }
            other if other.starts_with('-') => anyhow::bail!("unknown option (see --help)"),
            other => positional.push(other),
        }
    }
    let [service, base] = positional.as_slice() else {
        anyhow::bail!("expected <service> <test-url>");
    };
    ensure!(!target.token.is_empty(), "--token is required");
    target.base = base.trim_end_matches('/').to_owned();
    let acceptance = if acceptance {
        let label = label.context("--acceptance needs --target-build")?;
        ensure!(
            valid_target_label(&label),
            "--target-build needs 1–64 ASCII letters, digits, dots, underscores or hyphens"
        );
        Some(Acceptance {
            label,
            path: path.context("--acceptance needs --report")?,
        })
    } else {
        ensure!(
            label.is_none() && path.is_none(),
            "--target-build and --report require --acceptance"
        );
        None
    };
    Ok(Options {
        service: (*service).to_owned(),
        target,
        only,
        acceptance,
    })
}

#[must_use = "return the assessment exit status to the caller"]
async fn execute(options: Options) -> anyhow::Result<u8> {
    let suite = mako_conformance::suites::suite(&options.service).with_context(|| {
        let services: Vec<_> = mako_conformance::suites::SUITES
            .iter()
            .map(|s| s.service())
            .collect();
        format!("unknown service. Available suites: {}", services.join(", "))
    })?;
    if let Some(acceptance) = options.acceptance {
        let parent = acceptance
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = NamedTempFile::new_in(parent)
            .context("cannot create report in destination directory")?;
        let report = mako_conformance::run(suite, options.target, options.only).await?;
        let evidence = report.acceptance(&acceptance.label)?;
        serde_json::to_writer_pretty(&mut file, &evidence).context("cannot write evidence JSON")?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(acceptance.path)
            .map_err(|e| e.error)
            .context("cannot publish evidence file")?;
        println!("{evidence}");
        Ok(evidence.exit_code())
    } else {
        let report = mako_conformance::run(suite, options.target, options.only).await?;
        println!("{report}");
        Ok(u8::from(!report.gaps().is_empty()))
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = match parse(&args) {
        Ok(options) => execute(options).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
