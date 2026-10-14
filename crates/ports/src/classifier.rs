//! Typed classification with per-option probabilities: the [`Classifier`] port.
//!
//! It is modelled on Workers AI Clef (`@cf/cloudflare/clef`), which answers several typed
//! questions about one text in a single pass:
//!
//! - **`choice`**: pick one of several options, each described by its criteria. Returns a
//!   probability for every option.
//! - **`noul`**: a yes/no question. Returns `P(yes)` and `P(no)`; any remaining mass means
//!   "unknown".
//! - **`score`**: a value in `0.0..=1.0`.
//!
//! Uses include ticket triage (intent and the yes/no flags in one call), interpreting a
//! customer's reply to an offer, and the reply-confidence judge. The fallback adapter (JSON-mode
//! structured output) fills the same shapes with self-reported probabilities.
//!
//! [`Classification`] is serde-serializable, because each call runs in its own `ctx.run` and
//! is journaled.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::llm::Usage;

/// One question about the text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Stable ID used to find the answer, e.g. `intent` or `wants_human`.
    pub id: String,
    /// The question as asked to the model.
    pub text: String,
    /// Answer type.
    #[serde(flatten)]
    pub kind: QuestionKind,
}

impl Question {
    /// A choice question.
    pub fn choice(
        id: impl Into<String>,
        text: impl Into<String>,
        options: Vec<ChoiceOption>,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            kind: QuestionKind::Choice { options },
        }
    }

    /// A yes/no question.
    pub fn noul(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            kind: QuestionKind::Noul,
        }
    }

    /// A score question.
    pub fn score(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            kind: QuestionKind::Score,
        }
    }
}

/// The type of answer a question expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionKind {
    /// Exactly one of `options`.
    Choice {
        /// The options, with the criteria that select each one.
        options: Vec<ChoiceOption>,
    },
    /// Yes or no.
    Noul,
    /// A score in `0.0..=1.0`.
    Score,
}

/// One option of a choice question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// Stable option ID, e.g. an `Intent` wire name such as `cancel_order`.
    pub id: String,
    /// When to pick this option (for intents: `Intent::description()`).
    pub criteria: String,
}

impl ChoiceOption {
    /// An option with its criteria.
    pub fn new(id: impl Into<String>, criteria: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            criteria: criteria.into(),
        }
    }
}

/// The probability of one option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionProbability {
    /// [`ChoiceOption::id`].
    pub option: String,
    /// Probability in `0.0..=1.0`.
    pub p: f64,
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnswerValue {
    /// Answer to a choice question.
    Choice {
        /// The most probable option.
        choice: String,
        /// Every option's probability, most probable first.
        probabilities: Vec<OptionProbability>,
    },
    /// Answer to a yes/no question.
    Noul {
        /// `P(yes)`.
        yes: f64,
        /// `P(no)`.
        no: f64,
    },
    /// Answer to a score question.
    Score {
        /// Score in `0.0..=1.0`.
        score: f64,
    },
}

impl AnswerValue {
    /// For a choice: the probabilities of the top option and the runner-up (`0.0` if there is
    /// only one option). These feed `OrgSettings::should_escalate`.
    pub fn top_two(&self) -> Option<(f64, f64)> {
        let Self::Choice { probabilities, .. } = self else {
            return None;
        };
        let mut ps: Vec<f64> = probabilities.iter().map(|o| o.p).collect();
        ps.sort_by(|a, b| b.total_cmp(a));
        Some((
            ps.first().copied().unwrap_or(0.0),
            ps.get(1).copied().unwrap_or(0.0),
        ))
    }

    /// For a choice: the probability of `option` (`0.0` if absent).
    pub fn probability_of(&self, option: &str) -> Option<f64> {
        let Self::Choice { probabilities, .. } = self else {
            return None;
        };
        Some(
            probabilities
                .iter()
                .find(|o| o.option == option)
                .map_or(0.0, |o| o.p),
        )
    }
}

/// One answered question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// [`Question::id`].
    pub question_id: String,
    /// The answer.
    #[serde(flatten)]
    pub value: AnswerValue,
}

/// Answers to every question, in the order asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    /// The model that answered.
    pub model: String,
    /// One answer per question.
    pub answers: Vec<Answer>,
    /// Tokens used, if the provider reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl Classification {
    /// The answer to question `id`.
    pub fn answer(&self, id: &str) -> Option<&AnswerValue> {
        self.answers
            .iter()
            .find(|a| a.question_id == id)
            .map(|a| &a.value)
    }

    /// The chosen option of choice question `id`.
    pub fn choice(&self, id: &str) -> Option<&str> {
        match self.answer(id)? {
            AnswerValue::Choice { choice, .. } => Some(choice),
            _ => None,
        }
    }

    /// `P(yes)` of yes/no question `id`.
    pub fn yes(&self, id: &str) -> Option<f64> {
        match self.answer(id)? {
            AnswerValue::Noul { yes, .. } => Some(*yes),
            _ => None,
        }
    }

    /// The score of score question `id`.
    pub fn score(&self, id: &str) -> Option<f64> {
        match self.answer(id)? {
            AnswerValue::Score { score } => Some(*score),
            _ => None,
        }
    }
}

/// A classification call failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClassifierError {
    /// Network failure, timeout, 5xx or throttling. Retryable.
    #[error("classifier unavailable: {0}")]
    Unavailable(String),
    /// The request was rejected (no questions, empty options, …). Not retryable.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// The answer did not match the questions. Not retryable.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

impl ClassifierError {
    /// Whether retrying the same call may succeed.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

/// Answers typed questions about a text with probabilities.
#[async_trait]
pub trait Classifier: Send + Sync {
    /// Classifies `text` (the state: for triage, the ticket's latest message plus context).
    ///
    /// Adapters must return exactly one answer per question, in order, with a choice's
    /// probabilities sorted most probable first and summing to about 1.
    async fn classify(
        &self,
        text: &str,
        questions: &[Question],
    ) -> Result<Classification, ClassifierError>;
}

#[async_trait]
impl<T: Classifier + ?Sized> Classifier for Arc<T> {
    async fn classify(
        &self,
        text: &str,
        questions: &[Question],
    ) -> Result<Classification, ClassifierError> {
        (**self).classify(text, questions).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_shape_and_helpers() {
        let q = Question::choice(
            "intent",
            "What does the customer want?",
            vec![ChoiceOption::new("cancel_order", "Cancel an order.")],
        );
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "id": "intent",
                "text": "What does the customer want?",
                "type": "choice",
                "options": [{ "id": "cancel_order", "criteria": "Cancel an order." }],
            })
        );

        let c: Classification = serde_json::from_value(json!({
            "model": "@cf/cloudflare/clef",
            "answers": [
                { "question_id": "intent", "type": "choice", "choice": "cancel_order",
                  "probabilities": [{ "option": "cancel_order", "p": 0.8 }, { "option": "other", "p": 0.15 }] },
                { "question_id": "wants_human", "type": "noul", "yes": 0.1, "no": 0.85 },
                { "question_id": "anger", "type": "score", "score": 0.3 },
            ],
        }))
        .unwrap();
        assert_eq!(c.choice("intent"), Some("cancel_order"));
        assert_eq!(c.answer("intent").unwrap().top_two(), Some((0.8, 0.15)));
        assert_eq!(
            c.answer("intent")
                .unwrap()
                .probability_of("refund_or_return"),
            Some(0.0)
        );
        assert_eq!(c.yes("wants_human"), Some(0.1));
        assert_eq!(c.score("anger"), Some(0.3));
        assert_eq!(c.yes("intent"), None);
    }
}
