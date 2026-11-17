//! `mako-seed` — one Stadtwerk in a running estate, through its public APIs.
//!
//! ```text
//! mako-seed [seed] [--verify] [--compose]   # fill the estate, then (optionally) assert it
//! mako-seed verify [--compose]              # assert the outcomes and that every Desk source has content
//! mako-seed listen [--port 9199]            # print every CloudEvent the estate emits, verified
//! ```
//!
//! Every write is a request an integrator could make — REST, and makod's
//! EDIFACT door for what arrives from the market — judged by the same checks
//! and the same Cedar policies. A step whose natural key already exists is
//! left alone, so a second run changes nothing. `--compose` reaches each
//! service by its compose host name (`http://marktd:8180`) instead of
//! loopback.
#![deny(unsafe_code)]

mod api;
mod listen;
mod seed;
mod verify;
mod world;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("✗ mako-seed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[String]) -> anyhow::Result<()> {
    let compose = args.iter().any(|a| a == "--compose");
    let estate = api::Estate::new(compose)?;
    match args.first().map(String::as_str) {
        Some("listen") => listen::run(&args[1..]).await,
        Some("verify") => verify::run(&estate).await,
        Some("seed") | None => {
            seed::run(&estate).await?;
            if args.iter().any(|a| a == "--verify") {
                verify::run(&estate).await?;
            }
            Ok(())
        }
        Some(flag) if flag.starts_with("--") => {
            seed::run(&estate).await?;
            if args.iter().any(|a| a == "--verify") {
                verify::run(&estate).await?;
            }
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command {other:?} — seed, verify or listen"),
    }
}
