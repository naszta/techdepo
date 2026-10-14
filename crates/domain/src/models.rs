//! Which model answers each AI task: app-wide settings, changed by the operator on the Muhaha
//! page (`app_settings`, key `models`). When none are stored, the deployment's defaults apply
//! (`LLM_BACKEND`, `AI_*`). Each invocation reads them once, journaled, so a running workflow
//! keeps the models it started with.

use serde::{Deserialize, Serialize};

/// Longest model ID accepted.
pub const MAX_MODEL_ID_CHARS: usize = 200;

labeled_enum! {
    /// Who serves a model. Credentials are the deployment's (Worker secrets), never settings.
    pub enum ModelProvider {
        /// Deterministic offline models: scripted agents and a keyword classifier.
        Mock => ("mock", "Mock (offline)"),
        /// Cloudflare Workers AI through the Worker's `AI` binding.
        WorkersAi => ("workers-ai", "Cloudflare Workers AI"),
        /// Anthropic (`ANTHROPIC_API_KEY`).
        Anthropic => ("anthropic", "Anthropic"),
        /// OpenAI, Responses API (`OPENAI_API_KEY`).
        OpenAi => ("openai", "OpenAI"),
        /// Azure OpenAI, Chat Completions (`AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_RESOURCE`).
        /// Model IDs are deployment names.
        Azure => ("azure", "Azure OpenAI"),
        /// Azure OpenAI, Responses API: the same resource, deployments and secrets as
        /// [`ModelProvider::Azure`].
        AzureResponses => ("azure-responses", "Azure OpenAI (Responses)"),
        /// Google Gemini (`GEMINI_API_KEY`).
        Gemini => ("gemini", "Google Gemini"),
        /// OpenRouter, OpenAI-compatible (`OPENROUTER_API_KEY`).
        OpenRouter => ("openrouter", "OpenRouter"),
    }
}

impl ModelProvider {
    /// The secrets the provider needs, all of them; none for the mock and Workers AI (a
    /// binding).
    pub const fn secrets(self) -> &'static [&'static str] {
        match self {
            Self::Mock | Self::WorkersAi => &[],
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::OpenAi => &["OPENAI_API_KEY"],
            Self::Azure | Self::AzureResponses => {
                &["AZURE_OPENAI_API_KEY", "AZURE_OPENAI_RESOURCE"]
            }
            Self::Gemini => &["GEMINI_API_KEY"],
            Self::OpenRouter => &["OPENROUTER_API_KEY"],
        }
    }

    /// What to set for the provider, for messages: `the OPENAI_API_KEY secret`, `the
    /// AZURE_OPENAI_API_KEY and AZURE_OPENAI_RESOURCE secrets`; `None` without secrets.
    pub fn secrets_hint(self) -> Option<String> {
        match self.secrets() {
            [] => None,
            [one] => Some(format!("the {one} secret")),
            [init @ .., last] => Some(format!("the {} and {last} secrets", init.join(", "))),
        }
    }

    /// Whether `model` is a decision model (typed classification with calibrated
    /// probabilities, Workers AI Clef). Every other model classifies in JSON mode and reports
    /// its own probabilities.
    pub fn is_decision_model(self, model: &str) -> bool {
        self == Self::WorkersAi && model.starts_with("@cf/cloudflare/clef")
    }

    /// Models offered in the picker for `kind` (any other ID can be typed in).
    pub const fn suggested_models(self, kind: ModelKind) -> &'static [&'static str] {
        match (self, kind) {
            (Self::Mock, _) => &["mock"],
            (Self::WorkersAi, ModelKind::Chat) => &[
                "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
                "@cf/openai/gpt-oss-120b",
                "@cf/meta/llama-4-scout-17b-16e-instruct",
            ],
            (Self::WorkersAi, ModelKind::Classifier) => &[
                "@cf/cloudflare/clef",
                "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
            ],
            (Self::Anthropic, _) => &[
                "claude-haiku-4-5-20251001",
                "claude-sonnet-5",
                "claude-opus-5-5",
            ],
            (Self::OpenAi, _) => &["gpt-5-mini", "gpt-5.5"],
            // Deployment names are the operator's; these are the usual ones.
            (Self::Azure | Self::AzureResponses, _) => &["gpt-5-mini", "gpt-5.5"],
            (Self::Gemini, _) => &["gemini-2.5-flash", "gemini-2.5-pro"],
            (Self::OpenRouter, _) => &[
                "anthropic/claude-haiku-4.5",
                "openai/gpt-5-mini",
                "meta-llama/llama-3.3-70b-instruct",
            ],
        }
    }
}

labeled_enum! {
    /// What a model is used as.
    pub enum ModelKind {
        /// Chat with tool calling (agents).
        Chat => ("chat", "Chat"),
        /// Typed classification with probabilities.
        Classifier => ("classifier", "Classifier"),
    }
}

/// One model: the provider and its model ID, sent as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelChoice {
    /// Provider.
    pub provider: ModelProvider,
    /// The provider's model ID, e.g. `claude-sonnet-5` or `@cf/cloudflare/clef`.
    pub model: String,
}

impl ModelChoice {
    /// `model` on `provider`.
    pub fn new(provider: ModelProvider, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// The offline model.
    pub fn mock() -> Self {
        Self::new(ModelProvider::Mock, "mock")
    }

    /// `provider:model`, for logs and traces.
    pub fn address(&self) -> String {
        format!("{}:{}", self.provider.as_str(), self.model)
    }

    /// Parses a model address, `provider:model` (`azure:gpt-5-mini`, `workers-ai:@cf/…`;
    /// `cloudflare:` is Workers AI too, as in the evals' addresses). A bare model ID, without a
    /// `:`, is on `bare`. Only the first `:` separates: the rest is the model ID.
    pub fn parse(address: &str, bare: ModelProvider) -> Result<Self, ModelAddressError> {
        let address = address.trim();
        let (provider, model) = match address.split_once(':') {
            None => (bare, address),
            Some(("cloudflare", model)) => (ModelProvider::WorkersAi, model),
            Some((provider, model)) => match provider.parse() {
                Ok(provider) => (provider, model),
                Err(_) => return Err(ModelAddressError::UnknownProvider(provider.to_owned())),
            },
        };
        if model.is_empty() {
            return Err(ModelAddressError::EmptyModel(address.to_owned()));
        }
        Ok(Self::new(provider, model))
    }
}

/// A model address that does not parse ([`ModelChoice::parse`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelAddressError {
    /// The part before the `:` names no [`ModelProvider`].
    #[error("unknown model provider `{0}`")]
    UnknownProvider(String),
    /// Nothing after the provider.
    #[error("`{0}` names no model")]
    EmptyModel(String),
}

labeled_enum! {
    /// Each AI task that has its own model.
    pub enum ModelSlot {
        /// The product advisor agent (chat).
        ProductAdvisor => ("product_advisor", "Product advisor"),
        /// The order assistant agent (chat).
        OrderAssistant => ("order_assistant", "Order assistant"),
        /// Ticket triage by the router (classifier).
        Triage => ("triage", "Ticket triage"),
        /// Reading a customer's reply to an investigation offer (classifier).
        OfferReply => ("offer_reply", "Offer replies"),
        /// The judge scoring an agent's reply before it is sent (classifier).
        Judge => ("judge", "Reply judge"),
        /// Answers in JSON mode when a decision model returns an unusable answer (chat).
        ClassifierFallback => ("classifier_fallback", "Classifier fallback"),
    }
}

impl ModelSlot {
    /// What the slot's model is used as.
    pub const fn kind(self) -> ModelKind {
        match self {
            Self::ProductAdvisor | Self::OrderAssistant | Self::ClassifierFallback => {
                ModelKind::Chat
            }
            Self::Triage | Self::OfferReply | Self::Judge => ModelKind::Classifier,
        }
    }

    /// What the slot does, for the settings page.
    pub const fn description(self) -> &'static str {
        match self {
            Self::ProductAdvisor => {
                "Answers product and compatibility questions with catalog tools."
            }
            Self::OrderAssistant => {
                "Answers order, delivery and invoice questions with order tools."
            }
            Self::Triage => "Classifies every inbound ticket: intent, urgency, wants a person.",
            Self::OfferReply => "Reads the customer's answer to an investigation's offer.",
            Self::Judge => "Scores an agent's reply; the score gates sending it.",
            Self::ClassifierFallback => {
                "Classifies in JSON mode when a decision model's answer does not fit the questions. Optional."
            }
        }
    }
}

/// The model of every [`ModelSlot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    /// [`ModelSlot::ProductAdvisor`].
    pub product_advisor: ModelChoice,
    /// [`ModelSlot::OrderAssistant`].
    pub order_assistant: ModelChoice,
    /// [`ModelSlot::Triage`].
    pub triage: ModelChoice,
    /// [`ModelSlot::OfferReply`].
    pub offer_reply: ModelChoice,
    /// [`ModelSlot::Judge`].
    pub judge: ModelChoice,
    /// [`ModelSlot::ClassifierFallback`]; `None`: no fallback.
    #[serde(default)]
    pub classifier_fallback: Option<ModelChoice>,
}

impl ModelSettings {
    /// `chat` for the agents (and the fallback), `classifier` for classification.
    pub fn uniform(chat: ModelChoice, classifier: ModelChoice, fallback: bool) -> Self {
        Self {
            product_advisor: chat.clone(),
            order_assistant: chat.clone(),
            triage: classifier.clone(),
            offer_reply: classifier.clone(),
            judge: classifier,
            classifier_fallback: fallback.then_some(chat),
        }
    }

    /// The offline models everywhere.
    pub fn mock() -> Self {
        Self::uniform(ModelChoice::mock(), ModelChoice::mock(), false)
    }

    /// The model of `slot` (`None` only for an unset fallback).
    pub fn get(&self, slot: ModelSlot) -> Option<&ModelChoice> {
        match slot {
            ModelSlot::ProductAdvisor => Some(&self.product_advisor),
            ModelSlot::OrderAssistant => Some(&self.order_assistant),
            ModelSlot::Triage => Some(&self.triage),
            ModelSlot::OfferReply => Some(&self.offer_reply),
            ModelSlot::Judge => Some(&self.judge),
            ModelSlot::ClassifierFallback => self.classifier_fallback.as_ref(),
        }
    }

    /// Sets the model of `slot`; `None` clears the fallback and is ignored for the others.
    pub fn set(&mut self, slot: ModelSlot, choice: Option<ModelChoice>) {
        let required = match slot {
            ModelSlot::ProductAdvisor => &mut self.product_advisor,
            ModelSlot::OrderAssistant => &mut self.order_assistant,
            ModelSlot::Triage => &mut self.triage,
            ModelSlot::OfferReply => &mut self.offer_reply,
            ModelSlot::Judge => &mut self.judge,
            ModelSlot::ClassifierFallback => {
                self.classifier_fallback = choice;
                return;
            }
        };
        if let Some(choice) = choice {
            *required = choice;
        }
    }

    /// Every provider in use.
    pub fn providers(&self) -> Vec<ModelProvider> {
        let mut providers: Vec<ModelProvider> = ModelSlot::ALL
            .iter()
            .filter_map(|&slot| self.get(slot).map(|c| c.provider))
            .collect();
        providers.sort_unstable();
        providers.dedup();
        providers
    }

    /// Checks every model ID: present, at most [`MAX_MODEL_ID_CHARS`], no whitespace or
    /// control characters.
    pub fn validate(&self) -> Result<(), ModelSettingsError> {
        for &slot in ModelSlot::ALL {
            let Some(choice) = self.get(slot) else {
                continue;
            };
            let model = choice.model.as_str();
            if model.is_empty() {
                return Err(ModelSettingsError::EmptyModel(slot));
            }
            if model.chars().count() > MAX_MODEL_ID_CHARS {
                return Err(ModelSettingsError::ModelTooLong(slot));
            }
            if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(ModelSettingsError::InvalidModel(slot));
            }
        }
        Ok(())
    }
}

/// Model settings that cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModelSettingsError {
    /// No model ID.
    #[error("{} needs a model", .0.label())]
    EmptyModel(ModelSlot),
    /// The model ID is too long.
    #[error("{}: the model ID is longer than {MAX_MODEL_ID_CHARS} characters", .0.label())]
    ModelTooLong(ModelSlot),
    /// The model ID has whitespace or control characters.
    #[error("{}: a model ID has no spaces", .0.label())]
    InvalidModel(ModelSlot),
}

impl ModelSettingsError {
    /// The slot at fault.
    pub const fn slot(self) -> ModelSlot {
        match self {
            Self::EmptyModel(slot) | Self::ModelTooLong(slot) | Self::InvalidModel(slot) => slot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_settings_and_slots() {
        let chat = ModelChoice::new(ModelProvider::Anthropic, "claude-sonnet-5");
        let clef = ModelChoice::new(ModelProvider::WorkersAi, "@cf/cloudflare/clef");
        let mut s = ModelSettings::uniform(chat.clone(), clef.clone(), true);
        assert_eq!(s.get(ModelSlot::OrderAssistant), Some(&chat));
        assert_eq!(s.get(ModelSlot::Judge), Some(&clef));
        assert_eq!(s.get(ModelSlot::ClassifierFallback), Some(&chat));
        assert_eq!(
            s.providers(),
            [ModelProvider::WorkersAi, ModelProvider::Anthropic]
        );

        s.set(ModelSlot::ClassifierFallback, None);
        s.set(ModelSlot::Triage, None);
        assert_eq!(s.get(ModelSlot::ClassifierFallback), None);
        assert_eq!(
            s.get(ModelSlot::Triage),
            Some(&clef),
            "required slots keep theirs"
        );
        assert!(s.validate().is_ok());
        assert_eq!(ModelSettings::mock().providers(), [ModelProvider::Mock]);
    }

    #[test]
    fn validates_model_ids() {
        let mut s = ModelSettings::mock();
        s.judge.model = String::new();
        assert_eq!(
            s.validate(),
            Err(ModelSettingsError::EmptyModel(ModelSlot::Judge))
        );
        s.judge.model = "gpt 5".into();
        assert_eq!(
            s.validate().map_err(ModelSettingsError::slot),
            Err(ModelSlot::Judge)
        );
        s.judge.model = "x".repeat(MAX_MODEL_ID_CHARS + 1);
        assert!(s.validate().is_err());
    }

    #[test]
    fn parses_model_addresses() {
        let parse = |s: &str| ModelChoice::parse(s, ModelProvider::WorkersAi);
        assert_eq!(
            parse("azure:gpt-6-sol"),
            Ok(ModelChoice::new(ModelProvider::Azure, "gpt-6-sol"))
        );
        assert_eq!(
            parse(" azure-responses:foo "),
            Ok(ModelChoice::new(ModelProvider::AzureResponses, "foo"))
        );
        let llama = ModelChoice::new(ModelProvider::WorkersAi, "@cf/meta/llama-3.3-70b");
        assert_eq!(parse("@cf/meta/llama-3.3-70b").as_ref(), Ok(&llama), "bare");
        assert_eq!(
            parse("cloudflare:@cf/meta/llama-3.3-70b").as_ref(),
            Ok(&llama)
        );
        assert_eq!(parse(&llama.address()).as_ref(), Ok(&llama), "round trip");
        assert_eq!(
            parse("openrouter:meta-llama/llama-3.3-70b:free"),
            Ok(ModelChoice::new(
                ModelProvider::OpenRouter,
                "meta-llama/llama-3.3-70b:free"
            )),
            "only the first `:` separates"
        );
        assert_eq!(
            ModelChoice::parse("mock", ModelProvider::Mock),
            Ok(ModelChoice::mock())
        );
        assert_eq!(
            parse("bedrock:claude"),
            Err(ModelAddressError::UnknownProvider("bedrock".into()))
        );
        assert!(matches!(
            parse("anthropic:"),
            Err(ModelAddressError::EmptyModel(_))
        ));
        assert!(matches!(parse(""), Err(ModelAddressError::EmptyModel(_))));
    }

    #[test]
    fn decision_models_and_wire_shape() {
        assert!(ModelProvider::WorkersAi.is_decision_model("@cf/cloudflare/clef"));
        assert!(!ModelProvider::WorkersAi.is_decision_model("@cf/openai/gpt-oss-120b"));
        assert!(!ModelProvider::OpenRouter.is_decision_model("@cf/cloudflare/clef"));
        assert_eq!(ModelProvider::WorkersAi.secrets_hint(), None);
        assert_eq!(
            ModelProvider::Gemini.secrets_hint().as_deref(),
            Some("the GEMINI_API_KEY secret")
        );
        assert_eq!(
            ModelProvider::AzureResponses.secrets_hint().as_deref(),
            Some("the AZURE_OPENAI_API_KEY and AZURE_OPENAI_RESOURCE secrets")
        );
        assert_eq!(ModelSlot::Judge.kind(), ModelKind::Classifier);

        let s: ModelSettings = serde_json::from_value(serde_json::json!({
            "product_advisor": { "provider": "anthropic", "model": "claude-sonnet-5" },
            "order_assistant": { "provider": "openai", "model": "gpt-5-mini" },
            "triage": { "provider": "workers-ai", "model": "@cf/cloudflare/clef" },
            "offer_reply": { "provider": "gemini", "model": "gemini-2.5-flash" },
            "judge": { "provider": "openrouter", "model": "openai/gpt-5-mini" },
        }))
        .unwrap();
        assert_eq!(s.classifier_fallback, None, "the fallback is optional");
        assert_eq!(s.product_advisor.address(), "anthropic:claude-sonnet-5");
        let azure: ModelChoice = serde_json::from_value(
            serde_json::json!({ "provider": "azure-responses", "model": "gpt-5-mini" }),
        )
        .unwrap();
        assert_eq!(azure.provider, ModelProvider::AzureResponses);
    }
}
