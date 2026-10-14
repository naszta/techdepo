//! Developer tasks for TechDepó: `cargo run -p xtask -- <command>`.
//!
//! ```text
//! catalog validate        schema, plausibility, tiers, coverage and content checks
//! seed [--out PATH]       SQL that adds catalog products to every org's inventory
//! seed --local            apply migrations and that SQL to the local D1 (wrangler)
//! dev-vars                write apps/*/.dev.vars from the root .env (only the keys each reads)
//! eval classify|agents    the support evals (classification, agent scenarios) -> evals/reports/
//! ```

mod coverage;
mod dev_vars;
mod eval;
mod seed;
mod table;
mod tiers;
mod validate;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo run -p xtask -- <command>

commands:
  catalog validate        validate the embedded catalog: schema and plausibility, price tiers,
                          tier and platform coverage, rule coverage and content sections
  seed [--out PATH]       print (or write) D1 SQL that adds every catalog product to the
                          inventory of every provisioned org at its default stock
  seed --local            apply migrations and the seed SQL to the local D1 via wrangler
  dev-vars                write each worker's .dev.vars from the root .env (falls back to
                          .env.example), with only the keys that worker reads
  eval classify [options] run evals/classification.yaml through the router's triage: accuracy,
                          confusion matrix, calibration, threshold sweep
  eval agents [options]   run evals/agent_scenarios.yaml through the agent route pipeline
                          (real tools over a seeded SQLite shop, guardrails, judge)
      --backend mock|workers-ai   mock models (default), or real ones with the services' model
                                  variables (AI_AGENT_MODEL, AI_CLASSIFIER_MODEL)
      --agent-model ADDR          real chat model, <provider>:<model> (anthropic, openai, azure,
                                  azure-responses, gemini, cloudflare; a bare @cf/... ID is
                                  cloudflare)
      --classifier-model ADDR     real decision model (cloudflare:@cf/cloudflare/clef), or
                                  `none` for JSON-mode classification on the agent model
                                  (provider keys from the environment or .env)
      --only TEXT                 only cases/scenarios whose ID contains TEXT
      --file PATH / --out PATH    eval file / report path (default evals/reports/<eval>-<model>.md)
      -v, --verbose               print every case and agent text
  help                    show this message";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["catalog", "validate"] => validate::run(),
        ["seed", rest @ ..] => parse_seed(rest).and_then(seed::run),
        ["dev-vars"] => dev_vars::run(),
        ["eval", rest @ ..] => eval::parse(rest).and_then(eval::run),
        [] | ["help" | "-h" | "--help"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(format!("unknown command `{}`\n\n{USAGE}", args.join(" "))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_seed(args: &[&str]) -> Result<seed::Options, String> {
    let mut options = seed::Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--out" => {
                let path = args.next().ok_or("--out needs a path")?;
                options.out = Some(PathBuf::from(path));
            }
            "--local" => options.local = true,
            other => return Err(format!("unknown seed option `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}
