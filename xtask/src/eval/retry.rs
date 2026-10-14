//! Retries for the real models: the services retry a journaled step (Restate), the evals' inline
//! runtime does not, so a rate limit or a 5xx would end a scenario in an escalation. These
//! wrappers retry retryable errors with a short backoff, about as the services would.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use techdepo_ports::{
    ChatRequest, ChatResponse, Classification, Classifier, ClassifierError, Llm, LlmError, Question,
};

/// Attempts per call (the services' agent steps: 5).
const ATTEMPTS: u32 = 5;

/// The wait before attempt `n + 1` (0.5 s, 1 s, 2 s, …). Blocking is fine: the evals run one
/// call at a time.
fn backoff(attempt: u32) {
    std::thread::sleep(Duration::from_millis(250 << attempt.min(6)));
}

/// A chat model whose retryable errors are retried.
pub struct RetryLlm(pub Arc<dyn Llm>);

#[async_trait]
impl Llm for RetryLlm {
    fn default_model(&self) -> &str {
        self.0.default_model()
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut attempt = 1;
        loop {
            match self.0.chat(request).await {
                Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                    backoff(attempt);
                    attempt += 1;
                }
                other => return other,
            }
        }
    }
}

/// A classifier whose retryable errors are retried.
pub struct RetryClassifier(pub Arc<dyn Classifier>);

#[async_trait]
impl Classifier for RetryClassifier {
    async fn classify(
        &self,
        text: &str,
        questions: &[Question],
    ) -> Result<Classification, ClassifierError> {
        let mut attempt = 1;
        loop {
            match self.0.classify(text, questions).await {
                Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                    backoff(attempt);
                    attempt += 1;
                }
                other => return other,
            }
        }
    }
}
