//! `mako-conformance <service> <base-url> [options]` — exit 0 when every check
//! held, 1 on a gap, 2 on a usage error.

use std::process::ExitCode;
use std::time::Duration;

use mako_conformance::report::Runner;
use mako_conformance::suite::Target;

const USAGE: &str = "\
mako-conformance <service> <base-url> --token <bearer>
    [--refused-token <bearer>]   a principal the service refuses (403 / MCP refusal)
    [--inbound-secret <secret>]  the secret the service verifies inbound events with
    [--listen 127.0.0.1:9198]    where the kit receives deliveries
    [--callback <url>]           the URL the service delivers to (default http://<listen>/)
    [--wait 30]                  seconds to wait for deliveries
    [--only rest|events|mcp]

Run against a test deployment, never production.";

fn parse(args: &[String]) -> Result<(String, Target, Option<Runner>), String> {
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--token" => target.token = value()?,
            "--refused-token" => target.refused_token = Some(value()?),
            "--inbound-secret" => target.inbound_secret = Some(value()?),
            "--listen" => target.listen = value()?,
            "--callback" => target.callback = Some(value()?),
            "--wait" => {
                target.wait = Duration::from_secs(
                    value()?
                        .parse()
                        .map_err(|_| "--wait takes seconds".to_owned())?,
                );
            }
            "--only" => {
                let v = value()?;
                only = Some(
                    Runner::parse(&v)
                        .ok_or_else(|| format!("--only {v:?} is not rest, events or mcp"))?,
                );
            }
            "-h" | "--help" => return Err(String::new()),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => positional.push(other.to_owned()),
        }
    }
    let [service, base] = positional.as_slice() else {
        return Err("expected <service> <base-url>".to_owned());
    };
    if target.token.is_empty() {
        return Err("--token is required".to_owned());
    }
    target.base = base.trim_end_matches('/').to_owned();
    Ok((service.clone(), target, only))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (service, target, only) = match parse(&args) {
        Ok(p) => p,
        Err(why) => {
            if !why.is_empty() {
                eprintln!("{why}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let Some(suite) = mako_conformance::suites::suite(&service) else {
        let known: Vec<_> = mako_conformance::suites::SUITES
            .iter()
            .map(|s| s.service())
            .collect();
        eprintln!("no suite for {service:?} — known: {}", known.join(", "));
        return ExitCode::from(2);
    };
    match mako_conformance::run(suite, target, only).await {
        Ok(report) => {
            println!("{report}");
            if report.gaps().is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
