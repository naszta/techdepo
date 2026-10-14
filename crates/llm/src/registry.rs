//! [`ModelRegistry`]: the [`Models`] port over the providers a deployment can reach.
//!
//! A [`ModelChoice`] from the model settings becomes an adapter on the fly: chat models are
//! [`AllamaLlm`]; classifiers are a [`DecisionClassifier`] for decision models (Clef) and a
//! [`StructuredClassifier`] (JSON mode) for every other model, behind [`WithFallback`] when a
//! fallback model is set. The mock provider is always there. A provider without credentials
//! answers every call with a non-retryable error naming what is missing.

use std::collections::BTreeMap;
use std::sync::Arc;

use allama::{ChatProvider, DecisionProvider, DynChatProvider, DynDecisionProvider};
use async_trait::async_trait;
use techdepo_domain::{ModelChoice, ModelProvider, ModelSettings};
use techdepo_ports::{
    ChatRequest, ChatResponse, Classification, Classifier, ClassifierError, Llm, LlmError, Models,
    Question,
};

use crate::chat::AllamaLlm;
use crate::config::{Backend, LlmConfig};
use crate::decision::DecisionClassifier;
use crate::mock::{MockClassifier, MockLlm};
use crate::structured::{StructuredClassifier, WithFallback};

impl LlmConfig {
    /// The model settings this configuration stands for: what applies until the operator
    /// stores others.
    pub fn defaults(&self) -> ModelSettings {
        match self.backend {
            Backend::Mock => ModelSettings::mock(),
            Backend::WorkersAi => ModelSettings::uniform(
                self.agent_model.clone(),
                self.classifier_model.clone(),
                self.fallback_classifier,
            ),
        }
    }
}

/// The providers of a deployment, and its default model settings.
#[derive(Clone)]
pub struct ModelRegistry {
    defaults: ModelSettings,
    chat: BTreeMap<ModelProvider, DynChatProvider>,
    decisions: BTreeMap<ModelProvider, DynDecisionProvider>,
}

impl std::fmt::Debug for ModelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRegistry")
            .field("available", &self.available())
            .finish_non_exhaustive()
    }
}

impl ModelRegistry {
    /// Only the mock provider, with `defaults`.
    pub fn new(defaults: ModelSettings) -> Self {
        Self {
            defaults,
            chat: BTreeMap::new(),
            decisions: BTreeMap::new(),
        }
    }

    /// Serves `provider`'s chat models (and JSON-mode classification) through `chat`.
    #[must_use]
    pub fn with_chat<C>(mut self, provider: ModelProvider, chat: C) -> Self
    where
        C: ChatProvider + Send + Sync + 'static,
    {
        self.chat.insert(provider, DynChatProvider::new(chat));
        self
    }

    /// Serves `provider`'s decision models through `decisions`.
    #[must_use]
    pub fn with_decisions<D>(mut self, provider: ModelProvider, decisions: D) -> Self
    where
        D: DecisionProvider + Send + Sync + 'static,
    {
        self.decisions
            .insert(provider, DynDecisionProvider::new(decisions));
        self
    }

    /// JSON-mode classification on a chat model.
    fn structured(&self, choice: &ModelChoice) -> Arc<dyn Classifier> {
        match (choice.provider, self.chat.get(&choice.provider)) {
            (ModelProvider::Mock, _) => Arc::new(MockClassifier),
            (_, Some(chat)) => Arc::new(StructuredClassifier::new(chat.clone(), &choice.model)),
            (_, None) => Arc::new(NotConfigured(choice.clone())),
        }
    }
}

impl Models for ModelRegistry {
    fn available(&self) -> Vec<ModelProvider> {
        let mut providers = vec![ModelProvider::Mock];
        providers.extend(self.chat.keys().chain(self.decisions.keys()).copied());
        providers.sort_unstable();
        providers.dedup();
        providers
    }

    fn defaults(&self) -> ModelSettings {
        self.defaults.clone()
    }

    fn chat(&self, choice: &ModelChoice) -> Arc<dyn Llm> {
        match (choice.provider, self.chat.get(&choice.provider)) {
            (ModelProvider::Mock, _) => Arc::new(MockLlm),
            (_, Some(chat)) => Arc::new(AllamaLlm::new(chat.clone(), &choice.model)),
            (_, None) => Arc::new(NotConfigured(choice.clone())),
        }
    }

    fn classifier(
        &self,
        choice: &ModelChoice,
        fallback: Option<&ModelChoice>,
    ) -> Arc<dyn Classifier> {
        let primary: Arc<dyn Classifier> = if choice.provider.is_decision_model(&choice.model) {
            match self.decisions.get(&choice.provider) {
                Some(decisions) => {
                    Arc::new(DecisionClassifier::new(decisions.clone(), &choice.model))
                }
                None => Arc::new(NotConfigured(choice.clone())),
            }
        } else {
            self.structured(choice)
        };
        match fallback.filter(|f| *f != choice && choice.provider != ModelProvider::Mock) {
            Some(fallback) => Arc::new(WithFallback::new(primary, self.structured(fallback))),
            None => primary,
        }
    }
}

/// A model on a provider the deployment has no credentials for: every call fails, for good.
#[derive(Debug, Clone)]
struct NotConfigured(ModelChoice);

impl NotConfigured {
    fn message(&self) -> String {
        let provider = self.0.provider;
        match provider.secrets_hint() {
            Some(secrets) => format!(
                "{} is not configured in this deployment (set {secrets})",
                provider.label()
            ),
            None => format!("{} is not configured in this deployment", provider.label()),
        }
    }
}

#[async_trait]
impl Llm for NotConfigured {
    fn default_model(&self) -> &str {
        &self.0.model
    }

    async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::InvalidRequest(self.message()))
    }
}

#[async_trait]
impl Classifier for NotConfigured {
    async fn classify(
        &self,
        _text: &str,
        _questions: &[Question],
    ) -> Result<Classification, ClassifierError> {
        Err(ClassifierError::InvalidRequest(self.message()))
    }
}

#[cfg(test)]
mod tests {
    use allama::mock::{MockDecisionProvider, MockProvider};
    use futures::executor::block_on;
    use techdepo_ports::{ChatMessage, ChoiceOption};

    use super::*;

    fn questions() -> Vec<Question> {
        vec![Question::choice(
            "intent",
            "What does the customer want?",
            vec![
                ChoiceOption::new("cancel_order", "Cancel an order."),
                ChoiceOption::new("other", "Anything else."),
            ],
        )]
    }

    #[test]
    fn defaults_follow_the_backend() {
        assert_eq!(LlmConfig::default().defaults(), ModelSettings::mock());
        let workers_ai = LlmConfig {
            backend: Backend::WorkersAi,
            ..LlmConfig::default()
        };
        let d = workers_ai.defaults();
        assert_eq!(
            d.order_assistant,
            ModelChoice::new(ModelProvider::WorkersAi, LlmConfig::DEFAULT_AGENT_MODEL)
        );
        assert_eq!(
            d.judge,
            ModelChoice::new(
                ModelProvider::WorkersAi,
                LlmConfig::DEFAULT_CLASSIFIER_MODEL
            )
        );
        assert_eq!(d.classifier_fallback, Some(d.product_advisor.clone()));

        let azure = ModelChoice::new(ModelProvider::Azure, "gpt-6-sol");
        let d = LlmConfig {
            agent_model: azure.clone(),
            ..workers_ai
        }
        .defaults();
        assert_eq!(d.product_advisor, azure, "the address's provider");
        assert_eq!(d.triage.provider, ModelProvider::WorkersAi);
    }

    #[test]
    fn picks_adapters_by_choice() {
        let registry = ModelRegistry::new(ModelSettings::mock())
            .with_chat(ModelProvider::Anthropic, Arc::new(MockProvider::empty()))
            .with_chat(ModelProvider::WorkersAi, Arc::new(MockProvider::empty()))
            .with_decisions(ModelProvider::WorkersAi, MockDecisionProvider::empty());
        assert_eq!(
            registry.available(),
            [
                ModelProvider::Mock,
                ModelProvider::WorkersAi,
                ModelProvider::Anthropic
            ]
        );

        let sonnet = ModelChoice::new(ModelProvider::Anthropic, "claude-sonnet-5");
        assert_eq!(registry.chat(&sonnet).default_model(), "claude-sonnet-5");
        assert_eq!(
            registry.chat(&ModelChoice::mock()).default_model(),
            MockLlm::MODEL
        );

        // A provider without credentials fails for good, naming the secret.
        let gpt = ModelChoice::new(ModelProvider::OpenAi, "gpt-5-mini");
        let request = ChatRequest {
            messages: vec![ChatMessage::user("hi")],
            ..ChatRequest::default()
        };
        let err = block_on(registry.chat(&gpt).chat(&request)).unwrap_err();
        assert!(!err.is_retryable());
        assert!(err.to_string().contains("OPENAI_API_KEY"), "{err}");
        let err =
            block_on(registry.classifier(&gpt, None).classify("x", &questions())).unwrap_err();
        assert!(matches!(err, ClassifierError::InvalidRequest(_)), "{err}");

        // The mock classifier answers, fallback or not.
        let mock = block_on(
            registry
                .classifier(&ModelChoice::mock(), Some(&sonnet))
                .classify("Please cancel my order", &questions()),
        )
        .unwrap();
        assert_eq!(mock.model, MockClassifier::MODEL);
    }
}
