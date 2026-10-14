//! The models a deployment can reach: [`Models`] turns a [`ModelChoice`] from the model
//! settings (`techdepo_domain::ModelSettings`) into an [`Llm`] or a [`Classifier`].
//!
//! Choosing is cheap and never fails: a provider the deployment has no credentials for gives a
//! model whose every call fails with a non-retryable error, which the callers already handle
//! (an agent escalates, triage escalates). Which providers are reachable is
//! [`Models::available`], for the settings page.

use std::sync::Arc;

use techdepo_domain::{ModelChoice, ModelProvider, ModelSettings};

use crate::classifier::Classifier;
use crate::llm::Llm;

/// The models behind the model settings.
pub trait Models: Send + Sync {
    /// The providers this deployment has credentials (or a binding) for.
    fn available(&self) -> Vec<ModelProvider>;

    /// The settings in effect when none are stored: the deployment's configuration.
    fn defaults(&self) -> ModelSettings;

    /// The chat model `choice` names.
    fn chat(&self, choice: &ModelChoice) -> Arc<dyn Llm>;

    /// A classifier on `choice`: a decision model (calibrated probabilities), or any chat
    /// model in JSON mode. With a `fallback`, an unusable answer is asked again in JSON mode
    /// on the fallback.
    fn classifier(
        &self,
        choice: &ModelChoice,
        fallback: Option<&ModelChoice>,
    ) -> Arc<dyn Classifier>;
}

impl<T: Models + ?Sized> Models for Arc<T> {
    fn available(&self) -> Vec<ModelProvider> {
        (**self).available()
    }

    fn defaults(&self) -> ModelSettings {
        (**self).defaults()
    }

    fn chat(&self, choice: &ModelChoice) -> Arc<dyn Llm> {
        (**self).chat(choice)
    }

    fn classifier(
        &self,
        choice: &ModelChoice,
        fallback: Option<&ModelChoice>,
    ) -> Arc<dyn Classifier> {
        (**self).classifier(choice, fallback)
    }
}
