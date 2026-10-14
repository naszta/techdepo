//! `xtask eval classify|agents`: the support evals (support.md §10), run natively.
//!
//! - **`classify`** ([`classify`]): `evals/classification.yaml` through the router's triage
//!   (`router::triage_questions`, `Triage::read`, `decide` with the default org settings).
//!   Reports accuracy, the confusion matrix, calibration buckets and a threshold sweep.
//! - **`agents`** ([`agents`]): `evals/agent_scenarios.yaml` through the agent route pipeline
//!   (`agents::pipeline::answer`: loop, real tools over a seeded SQLite shop, guardrails, judge)
//!   with assertions on outcome, tool calls, grounding and recommendations.
//!
//! Backends ([`Backend`]): `mock` (the offline `MockClassifier` / `MockLlm`, the default) or
//! real models, given as `allama-providers` model addresses (`<provider>:<model>`):
//!
//! - `--agent-model` (else `AI_AGENT_MODEL`, else the services' default): the chat model of
//!   the agents and of JSON-mode classification, e.g. `anthropic:claude-haiku-4-5`,
//!   `openai:gpt-5-mini`, `azure:<deployment>` (`azure-responses:` for the Responses API),
//!   `gemini:gemini-2.5-flash`, `cloudflare:@cf/meta/llama-3.3-70b-instruct-fp8-fast`.
//! - `--classifier-model` (else `AI_CLASSIFIER_MODEL`, else Clef): a decision model
//!   (`cloudflare:@cf/cloudflare/clef`), or `none` for JSON-mode classification on the agent
//!   model. `AI_FALLBACK_CLASSIFIER` applies as in the services worker.
//!
//! A bare model ID (`@cf/…`, as the services worker's variables hold it) means `cloudflare:`.
//! `--backend workers-ai` is real models with those defaults. Each provider reads its usual
//! variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AZURE_OPENAI_API_KEY` +
//! `AZURE_OPENAI_ENDPOINT` or `AZURE_OPENAI_RESOURCE`, `GEMINI_API_KEY`, `CLOUDFLARE_ACCOUNT_ID` +
//! `CLOUDFLARE_API_TOKEN`) from the environment or the root `.env`. The providers are xtask's
//! features (all on by default).
//!
//! Each run prints a summary and writes a markdown report to `evals/reports/` (one file per
//! eval, backend and model, overwritten). It fails when a metric is below the minimums in the
//! eval file.

mod agents;
mod classify;
mod replies;
mod retry;
mod shop;
mod stats;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allama::DynDecisionProvider;
use allama_providers::ProviderSettings;
use techdepo_llm::{Backend as LlmBackend, LlmConfig, LlmPorts};

use crate::dev_vars;

/// Which models answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The offline models.
    Mock,
    /// Real models, from model addresses.
    Models,
}

impl Backend {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "mock" => Ok(Self::Mock),
            "models" | "workers-ai" | "workers_ai" => Ok(Self::Models),
            other => Err(format!(
                "unknown backend `{other}` (expected `mock`, `models` or `workers-ai`)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Models => "models",
        }
    }
}

/// `--classifier-model none`: classify in JSON mode on the agent model.
const NO_DECISION_MODEL: &str = "none";

/// A model address for `allama-providers`: a bare model ID, or one on `workers-ai:` (the
/// services' name for it), is Workers AI over HTTP (`cloudflare:`).
fn model_address(model: &str) -> String {
    let model = model.trim();
    match model.split_once(':') {
        None => format!("cloudflare:{model}"),
        Some(("workers-ai", id)) => format!("cloudflare:{id}"),
        Some(_) => model.to_owned(),
    }
}

/// Which eval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Triage classification.
    Classify,
    /// Agent scenarios.
    Agents,
}

/// Command-line options.
#[derive(Debug, Clone)]
pub struct Options {
    kind: Kind,
    backend: Backend,
    file: Option<PathBuf>,
    out: Option<PathBuf>,
    agent_model: Option<String>,
    classifier_model: Option<String>,
    only: Option<String>,
    verbose: bool,
}

/// Parses `eval <classify|agents> [options]`.
pub fn parse(args: &[&str]) -> Result<Options, String> {
    let (kind, rest) = match args {
        ["classify", rest @ ..] => (Kind::Classify, rest),
        ["agents", rest @ ..] => (Kind::Agents, rest),
        _ => return Err("eval needs `classify` or `agents`".to_owned()),
    };
    let mut backend = None;
    let mut options = Options {
        kind,
        backend: Backend::Mock,
        file: None,
        out: None,
        agent_model: None,
        classifier_model: None,
        only: None,
        verbose: false,
    };
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .map(|v| (*v).to_owned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match *arg {
            "--backend" => backend = Some(Backend::parse(&value("--backend")?)?),
            "--file" => options.file = Some(PathBuf::from(value("--file")?)),
            "--out" => options.out = Some(PathBuf::from(value("--out")?)),
            "--agent-model" => options.agent_model = Some(value("--agent-model")?),
            "--classifier-model" => options.classifier_model = Some(value("--classifier-model")?),
            "--only" => options.only = Some(value("--only")?),
            "-v" | "--verbose" => options.verbose = true,
            other => return Err(format!("unknown eval option `{other}`")),
        }
    }
    let models_given = options.agent_model.is_some() || options.classifier_model.is_some();
    options.backend = match backend {
        Some(Backend::Mock) if models_given => {
            return Err("--backend mock takes no --agent-model or --classifier-model".to_owned());
        }
        Some(backend) => backend,
        None if models_given => Backend::Models,
        None => Backend::Mock,
    };
    Ok(options)
}

/// Runs the eval.
pub fn run(options: Options) -> Result<(), String> {
    let root = dev_vars::root();
    let models = Models::build(&options, &root)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the async runtime: {e}"))?;
    let (default_file, slug) = match options.kind {
        Kind::Classify => ("evals/classification.yaml", "classify"),
        Kind::Agents => ("evals/agent_scenarios.yaml", "agents"),
    };
    let file = options
        .file
        .clone()
        .unwrap_or_else(|| root.join(default_file));
    let source = std::fs::read_to_string(&file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let out = options.out.clone().unwrap_or_else(|| {
        root.join("evals/reports")
            .join(format!("{slug}-{}.md", models.slug(options.kind)))
    });

    let run = Run {
        options: &options,
        models: &models,
        source_name: display_path(&file, &root),
    };
    let report = match options.kind {
        Kind::Classify => runtime.block_on(classify::run(&run, &source))?,
        Kind::Agents => runtime.block_on(agents::run(&run, &source))?,
    };

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&out, &report.markdown)
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    println!("\nreport: {}", display_path(&out, &root));

    if report.failures.is_empty() {
        println!("eval {slug} ({}): ok", options.backend.as_str());
        Ok(())
    } else {
        Err(format!(
            "eval {slug} ({}) is below its minimums: {}",
            options.backend.as_str(),
            report.failures.join("; ")
        ))
    }
}

/// One value or a list of acceptable values (`intent: cancel_order` or `[cancel_order, other]`).
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// One value.
    One(T),
    /// Any of these.
    Many(Vec<T>),
}

impl<T: Copy> OneOrMany<T> {
    /// The values.
    pub fn to_vec(&self) -> Vec<T> {
        match self {
            Self::One(v) => vec![*v],
            Self::Many(vs) => vs.clone(),
        }
    }
}

/// What one eval produced.
pub struct Report {
    /// The markdown report.
    pub markdown: String,
    /// Minimums that were not met.
    pub failures: Vec<String>,
}

/// Shared inputs of one eval run.
pub struct Run<'a> {
    /// The options.
    pub options: &'a Options,
    /// The models.
    pub models: &'a Models,
    /// The eval file, relative to the repository root.
    pub source_name: String,
}

impl Run<'_> {
    /// The report header: what ran against what.
    pub fn header(&self, title: &str) -> String {
        let now = time::OffsetDateTime::now_utc();
        let date = now
            .format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute] UTC"
            ))
            .unwrap_or_default();
        format!(
            "# {title}\n\n\
             | | |\n|---|---|\n\
             | Backend | `{}` |\n\
             | Classifier | `{}` |\n\
             | Agent model | `{}` |\n\
             | Eval file | `{}` |\n\
             | Run at | {date} |\n\n\
             Generated by `cargo run -p xtask -- eval {}`; do not edit by hand.\n\n",
            self.options.backend.as_str(),
            self.models.classifier_name,
            self.models.agent_name,
            self.source_name,
            match self.options.kind {
                Kind::Classify => format!("classify --backend {}", self.options.backend.as_str()),
                Kind::Agents => format!("agents --backend {}", self.options.backend.as_str()),
            },
        )
    }

    /// Whether `id` is selected by `--only` (a substring).
    pub fn selected(&self, id: &str) -> bool {
        self.options.only.as_deref().is_none_or(|o| id.contains(o))
    }
}

/// The models under evaluation.
pub struct Models {
    /// Chat and classifier ports.
    pub ports: LlmPorts,
    /// The classifier's name (with the fallback, if any).
    pub classifier_name: String,
    /// The agent model's name.
    pub agent_name: String,
}

impl Models {
    fn build(options: &Options, root: &Path) -> Result<Self, String> {
        match options.backend {
            Backend::Mock => Ok(Self {
                ports: LlmPorts::mock(),
                classifier_name: techdepo_llm::MockClassifier::MODEL.to_owned(),
                agent_name: techdepo_llm::MockLlm::MODEL.to_owned(),
            }),
            Backend::Models => Self::real(options, root),
        }
    }

    /// Real models through `allama-providers`, configured like the services worker.
    fn real(options: &Options, root: &Path) -> Result<Self, String> {
        export_dotenv(root);
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let agent = model_address(
            &options
                .agent_model
                .clone()
                .or_else(|| env("AI_AGENT_MODEL"))
                .unwrap_or_else(|| LlmConfig::DEFAULT_AGENT_MODEL.to_owned()),
        );
        let classifier = options
            .classifier_model
            .clone()
            .or_else(|| env("AI_CLASSIFIER_MODEL"))
            .unwrap_or_else(|| LlmConfig::DEFAULT_CLASSIFIER_MODEL.to_owned());
        let classifier =
            (classifier.trim() != NO_DECISION_MODEL).then(|| model_address(&classifier));

        // The retry wrappers below are the only retries, as the services' journaled steps are.
        let settings = ProviderSettings::new().retry(None);
        let (chat, agent_model) = allama_providers::chat::from_model_with(&agent, &settings)
            .map_err(|e| format!("agent model `{agent}`: {e}"))?;
        let decisions: Option<(DynDecisionProvider, String)> = classifier
            .as_deref()
            .map(|address| {
                allama_providers::decision::from_model_with(address, &settings)
                    .map_err(|e| format!("classifier model `{address}`: {e}"))
            })
            .transpose()?;

        // `with_providers` serves the model IDs with the providers above, whatever the address
        // says; the prefix only keeps a `:` in an ID from reading as a provider.
        let mut overrides: BTreeMap<&str, String> = BTreeMap::new();
        overrides.insert("LLM_BACKEND", "workers-ai".to_owned());
        overrides.insert("AI_AGENT_MODEL", format!("workers-ai:{agent_model}"));
        if let Some((_, model)) = &decisions {
            overrides.insert("AI_CLASSIFIER_MODEL", format!("workers-ai:{model}"));
        }
        let config =
            LlmConfig::from_lookup(|name| overrides.get(name).cloned().or_else(|| env(name)))
                .map_err(|e| e.to_string())?;
        debug_assert_eq!(config.backend, LlmBackend::WorkersAi);
        let fallback = config.fallback_classifier;
        let ports = config.with_providers(chat, decisions.map(|(provider, _)| provider));
        let ports = LlmPorts {
            llm: Arc::new(retry::RetryLlm(ports.llm)),
            classifier: Arc::new(retry::RetryClassifier(ports.classifier)),
        };
        Ok(Self {
            ports,
            classifier_name: match &classifier {
                None => format!("{agent} (JSON mode)"),
                Some(address) if fallback => format!("{address} (fallback: {agent} JSON mode)"),
                Some(address) => address.clone(),
            },
            agent_name: agent,
        })
    }

    /// The report file's suffix: the backend, plus the model for real backends.
    fn slug(&self, kind: Kind) -> String {
        let model = match kind {
            Kind::Classify => self.classifier_name.split(' ').next().unwrap_or(""),
            Kind::Agents => &self.agent_name,
        };
        if model.starts_with("mock") {
            return "mock".to_owned();
        }
        let (provider, model) = model.split_once(':').unwrap_or(("models", model));
        let model = model.rsplit('/').next().unwrap_or(model);
        let model: String = model
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("{provider}-{model}")
    }
}

/// Exports the root `.env`'s keys the environment does not set, so the providers' `from_env`
/// sees them (devenv loads `.env` itself; a plain `cargo run` does not).
fn export_dotenv(root: &Path) {
    let Ok(text) = std::fs::read_to_string(root.join(".env")) else {
        return;
    };
    for (key, value) in dev_vars::parse(&text) {
        if value.trim().is_empty() || std::env::var_os(&key).is_some() {
            continue;
        }
        // SAFETY: called before the async runtime starts, while xtask has a single thread.
        unsafe { std::env::set_var(&key, value) };
    }
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_addresses_for_allama_providers() {
        assert_eq!(
            model_address("@cf/cloudflare/clef"),
            "cloudflare:@cf/cloudflare/clef"
        );
        assert_eq!(
            model_address("workers-ai:@cf/cloudflare/clef"),
            "cloudflare:@cf/cloudflare/clef"
        );
        assert_eq!(model_address(" azure-responses:foo"), "azure-responses:foo");
    }
}
