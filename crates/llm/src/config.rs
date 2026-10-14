//! Picking the backend from configuration (`LLM_BACKEND`).

use std::str::FromStr;
use std::sync::Arc;

use allama::{ChatProvider, DecisionProvider};
use techdepo_domain::{ModelAddressError, ModelChoice, ModelProvider};
use techdepo_ports::{Classifier, Llm};

use crate::budget::BudgetLimits;
use crate::chat::AllamaLlm;
use crate::decision::DecisionClassifier;
use crate::mock::{MockClassifier, MockLlm};
use crate::structured::{StructuredClassifier, WithFallback};

/// Which models answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    /// Deterministic offline models (local development, demos without an account).
    #[default]
    Mock,
    /// Real models: the `AI_*` model addresses, Workers AI unless they name another provider
    /// (Clef for classification, a function-calling model for agents by default).
    WorkersAi,
}

impl FromStr for Backend {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "mock" => Ok(Self::Mock),
            "workers-ai" | "workers_ai" => Ok(Self::WorkersAi),
            other => Err(ConfigError::UnknownBackend(other.to_owned())),
        }
    }
}

/// The configuration is unusable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// `LLM_BACKEND` names no backend.
    #[error("LLM_BACKEND must be `mock` or `workers-ai`, not `{0}`")]
    UnknownBackend(String),
    /// A number does not parse.
    #[error("{0} must be a whole number of tokens")]
    InvalidNumber(&'static str),
    /// A model address does not parse.
    #[error("{0}: {1}")]
    InvalidModel(&'static str, ModelAddressError),
    /// The backend cannot be built here.
    #[error("{0}")]
    Unavailable(String),
}

/// Model configuration, from the worker's environment.
///
/// | Variable | Default |
/// |---|---|
/// | `LLM_BACKEND` | `mock` |
/// | `AI_AGENT_MODEL` | `@cf/meta/llama-3.3-70b-instruct-fp8-fast` |
/// | `AI_CLASSIFIER_MODEL` | `@cf/cloudflare/clef` |
///
/// The models are addresses ([`ModelChoice::parse`]): `anthropic:claude-sonnet-5`,
/// `azure-responses:<deployment>`; a bare model ID is Workers AI.
/// | `AI_FALLBACK_CLASSIFIER` | `on`: JSON-mode classification with the agent model when Clef fails |
/// | `LLM_DAILY_TOKEN_BUDGET_PER_ORG` | `200000` (`0` = unlimited) |
/// | `LLM_DAILY_TOKEN_BUDGET_GLOBAL` | `5000000` (`0` = unlimited) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConfig {
    /// Backend.
    pub backend: Backend,
    /// Chat model for agents.
    pub agent_model: ModelChoice,
    /// Decision model for classification.
    pub classifier_model: ModelChoice,
    /// Fall back to structured output when the decision model fails.
    pub fallback_classifier: bool,
    /// Daily token budgets.
    pub budget: BudgetLimits,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Mock,
            agent_model: ModelChoice::new(ModelProvider::WorkersAi, Self::DEFAULT_AGENT_MODEL),
            classifier_model: ModelChoice::new(
                ModelProvider::WorkersAi,
                Self::DEFAULT_CLASSIFIER_MODEL,
            ),
            fallback_classifier: true,
            budget: BudgetLimits::default(),
        }
    }
}

/// The models a backend provides.
#[derive(Clone)]
pub struct LlmPorts {
    /// Chat (agents).
    pub llm: Arc<dyn Llm>,
    /// Typed classification (triage, reply interpretation, the judge).
    pub classifier: Arc<dyn Classifier>,
}

impl std::fmt::Debug for LlmPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmPorts")
            .field("llm", &self.llm.default_model())
            .finish_non_exhaustive()
    }
}

impl LlmPorts {
    /// The offline models.
    pub fn mock() -> Self {
        Self {
            llm: Arc::new(MockLlm),
            classifier: Arc::new(MockClassifier),
        }
    }
}

impl LlmConfig {
    /// The default agent model on Workers AI (architecture.md §6).
    pub const DEFAULT_AGENT_MODEL: &'static str = "@cf/meta/llama-3.3-70b-instruct-fp8-fast";
    /// The default decision model on Workers AI.
    pub const DEFAULT_CLASSIFIER_MODEL: &'static str = "@cf/cloudflare/clef";

    /// Reads the configuration through `get` (an env lookup); unset or empty means default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &str| get(name).filter(|v| !v.trim().is_empty());
        let defaults = Self::default();
        let tokens = |name: &'static str, default: u64| match get(name) {
            Some(v) => v
                .trim()
                .parse::<u64>()
                .map_err(|_| ConfigError::InvalidNumber(name)),
            None => Ok(default),
        };
        let model = |name: &'static str, default: ModelChoice| match get(name) {
            Some(v) => ModelChoice::parse(&v, ModelProvider::WorkersAi)
                .map_err(|e| ConfigError::InvalidModel(name, e)),
            None => Ok(default),
        };
        Ok(Self {
            backend: get("LLM_BACKEND")
                .map(|v| v.parse::<Backend>())
                .transpose()?
                .unwrap_or_default(),
            agent_model: model("AI_AGENT_MODEL", defaults.agent_model)?,
            classifier_model: model("AI_CLASSIFIER_MODEL", defaults.classifier_model)?,
            fallback_classifier: get("AI_FALLBACK_CLASSIFIER")
                .is_none_or(|v| !matches!(v.trim(), "off" | "false" | "0")),
            budget: BudgetLimits {
                per_org_daily_tokens: tokens(
                    "LLM_DAILY_TOKEN_BUDGET_PER_ORG",
                    defaults.budget.per_org_daily_tokens,
                )?,
                global_daily_tokens: tokens(
                    "LLM_DAILY_TOKEN_BUDGET_GLOBAL",
                    defaults.budget.global_daily_tokens,
                )?,
            },
        })
    }

    /// The ports for this configuration, with `chat` and `decisions` as the real providers
    /// (ignored for [`Backend::Mock`]) of the models' IDs: their providers are not looked at.
    /// Outside a Worker (evals), pass providers built by `allama-providers`. Without
    /// `decisions`, classification is JSON mode on the agent model ([`StructuredClassifier`])
    /// and `classifier_model` is unused.
    pub fn with_providers<C, D>(&self, chat: C, decisions: Option<D>) -> LlmPorts
    where
        C: ChatProvider + Clone + Send + Sync + 'static,
        D: DecisionProvider + Send + Sync + 'static,
    {
        if self.backend == Backend::Mock {
            return LlmPorts::mock();
        }
        let agent_model = &self.agent_model.model;
        let structured = || Arc::new(StructuredClassifier::new(chat.clone(), agent_model));
        let classifier: Arc<dyn Classifier> = match decisions {
            None => structured(),
            Some(decisions) => {
                let clef: Arc<dyn Classifier> = Arc::new(DecisionClassifier::new(
                    decisions,
                    &self.classifier_model.model,
                ));
                if self.fallback_classifier {
                    Arc::new(WithFallback::new(clef, structured()))
                } else {
                    clef
                }
            }
        };
        LlmPorts {
            llm: Arc::new(AllamaLlm::new(chat, agent_model)),
            classifier,
        }
    }

    /// The ports inside a Worker: Workers AI through the `AI` binding.
    ///
    /// Needs `[ai] binding = "AI"` in `wrangler.toml` for [`Backend::WorkersAi`].
    #[cfg(target_arch = "wasm32")]
    pub fn build(&self, env: &worker::Env) -> Result<LlmPorts, ConfigError> {
        match self.backend {
            Backend::Mock => Ok(LlmPorts::mock()),
            Backend::WorkersAi => {
                let provider = allama_cloudflare_worker::Provider::from_env(env, "AI")
                    .map_err(|e| ConfigError::Unavailable(e.to_string()))?;
                let provider = Arc::new(provider);
                Ok(self.with_providers(provider.clone(), Some(provider)))
            }
        }
    }

    /// The ports outside a Worker: only [`Backend::Mock`]; use
    /// [`with_providers`](Self::with_providers) for Workers AI over HTTP.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn build(&self) -> Result<LlmPorts, ConfigError> {
        match self.backend {
            Backend::Mock => Ok(LlmPorts::mock()),
            Backend::WorkersAi => Err(ConfigError::Unavailable(
                "LLM_BACKEND=workers-ai uses the Worker's AI binding; outside a Worker, build \
                 the providers over HTTP and call `LlmConfig::with_providers`"
                    .to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn reads_the_environment() {
        assert_eq!(
            LlmConfig::from_lookup(lookup(&[])).unwrap(),
            LlmConfig::default()
        );
        let config = LlmConfig::from_lookup(lookup(&[
            ("LLM_BACKEND", "workers-ai"),
            ("AI_AGENT_MODEL", "@cf/openai/gpt-oss-120b"),
            ("AI_CLASSIFIER_MODEL", ""),
            ("AI_FALLBACK_CLASSIFIER", "off"),
            ("LLM_DAILY_TOKEN_BUDGET_PER_ORG", "1000"),
        ]))
        .unwrap();
        assert_eq!(config.backend, Backend::WorkersAi);
        assert_eq!(
            config.agent_model,
            ModelChoice::new(ModelProvider::WorkersAi, "@cf/openai/gpt-oss-120b")
        );
        assert_eq!(
            config.classifier_model,
            LlmConfig::default().classifier_model
        );
        assert!(!config.fallback_classifier);
        assert_eq!(config.budget.per_org_daily_tokens, 1000);
        assert_eq!(config.budget.global_daily_tokens, 5_000_000);

        assert!(LlmConfig::from_lookup(lookup(&[("LLM_BACKEND", "openai")])).is_err());

        // Model addresses name their provider.
        let config = LlmConfig::from_lookup(lookup(&[
            ("LLM_BACKEND", "workers-ai"),
            ("AI_AGENT_MODEL", "azure-responses:gpt-6-sol"),
            ("AI_CLASSIFIER_MODEL", "cloudflare:@cf/cloudflare/clef"),
        ]))
        .unwrap();
        assert_eq!(
            config.agent_model,
            ModelChoice::new(ModelProvider::AzureResponses, "gpt-6-sol")
        );
        assert_eq!(
            config.classifier_model,
            LlmConfig::default().classifier_model
        );
        let err =
            LlmConfig::from_lookup(lookup(&[("AI_AGENT_MODEL", "bedrock:claude")])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "AI_AGENT_MODEL: unknown model provider `bedrock`"
        );
        assert!(
            LlmConfig::from_lookup(lookup(&[("LLM_DAILY_TOKEN_BUDGET_GLOBAL", "lots")])).is_err()
        );
    }

    // The native `build()` (in a Worker it takes the `Env` with the AI binding).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn builds_ports() {
        use allama::mock::{MockDecisionProvider, MockProvider};

        let mock = LlmConfig::default().build().unwrap();
        assert_eq!(mock.llm.default_model(), MockLlm::MODEL);

        let workers_ai = LlmConfig {
            backend: Backend::WorkersAi,
            ..LlmConfig::default()
        };
        assert!(
            workers_ai.build().is_err(),
            "no AI binding outside a Worker"
        );
        let ports = workers_ai.with_providers(
            Arc::new(MockProvider::empty()),
            Some(MockDecisionProvider::empty()),
        );
        assert_eq!(ports.llm.default_model(), LlmConfig::DEFAULT_AGENT_MODEL);
    }
}
