//! The ports the services run on ([`Ports`]), and stand-ins for the model port.
//!
//! [`Unconfigured`] implements `Models`, `Llm` and `Classifier` by failing with a non-retryable
//! "not configured" error; tests use it for ports they do not exercise. [`FixedModels`] answers
//! every model choice with the same chat model and classifier (tests, evals). Org membership
//! (`MemberDirectory`) is not a services port: only mail-in checks senders (support.md §1).

use std::sync::Arc;

use async_trait::async_trait;
use techdepo_domain::{ModelChoice, ModelProvider, ModelSettings};
use techdepo_ports::{
    ChatRequest, ChatResponse, Classification, Classifier, ClassifierError, Llm, LlmError, Mailer,
    Models, Question, SqlExecutor,
};

/// Every port a component may use.
#[derive(Clone)]
pub struct Ports {
    /// D1 (or SQLite).
    pub sql: Arc<dyn SqlExecutor>,
    /// The mail transport (only the `Email` service calls it).
    pub mailer: Arc<dyn Mailer>,
    /// The models (agents, triage, the judge, offer replies), picked per invocation from the
    /// model settings.
    pub models: Arc<dyn Models>,
}

impl Ports {
    /// Database and mailer; the models [`Unconfigured`].
    pub fn new(sql: Arc<dyn SqlExecutor>, mailer: Arc<dyn Mailer>) -> Self {
        Self {
            sql,
            mailer,
            models: Arc::new(Unconfigured),
        }
    }
}

/// A port that is not configured yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unconfigured;

const NOT_CONFIGURED: &str = "not configured in this deployment";

impl Models for Unconfigured {
    fn available(&self) -> Vec<ModelProvider> {
        Vec::new()
    }

    fn defaults(&self) -> ModelSettings {
        ModelSettings::mock()
    }

    fn chat(&self, _choice: &ModelChoice) -> Arc<dyn Llm> {
        Arc::new(Self)
    }

    fn classifier(
        &self,
        _choice: &ModelChoice,
        _fallback: Option<&ModelChoice>,
    ) -> Arc<dyn Classifier> {
        Arc::new(Self)
    }
}

#[async_trait]
impl Llm for Unconfigured {
    fn default_model(&self) -> &str {
        "unconfigured"
    }

    async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::InvalidRequest(NOT_CONFIGURED.to_owned()))
    }
}

#[async_trait]
impl Classifier for Unconfigured {
    async fn classify(
        &self,
        _text: &str,
        _questions: &[Question],
    ) -> Result<Classification, ClassifierError> {
        Err(ClassifierError::InvalidRequest(NOT_CONFIGURED.to_owned()))
    }
}

/// One chat model and one classifier for every choice: the settings pick nothing. Its
/// defaults are the mock settings, and it reports the mock provider only.
#[derive(Clone)]
pub struct FixedModels {
    llm: Arc<dyn Llm>,
    classifier: Arc<dyn Classifier>,
}

impl FixedModels {
    /// `llm` for chat, `classifier` for classification.
    pub fn new(llm: Arc<dyn Llm>, classifier: Arc<dyn Classifier>) -> Self {
        Self { llm, classifier }
    }
}

impl Models for FixedModels {
    fn available(&self) -> Vec<ModelProvider> {
        vec![ModelProvider::Mock]
    }

    fn defaults(&self) -> ModelSettings {
        ModelSettings::mock()
    }

    fn chat(&self, _choice: &ModelChoice) -> Arc<dyn Llm> {
        self.llm.clone()
    }

    fn classifier(
        &self,
        _choice: &ModelChoice,
        _fallback: Option<&ModelChoice>,
    ) -> Arc<dyn Classifier> {
        self.classifier.clone()
    }
}
