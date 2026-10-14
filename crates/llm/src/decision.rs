//! [`Classifier`] over any allama [`DecisionProvider`]: Workers AI Clef through the AI binding on
//! Workers, Clef over HTTP (System One) for native evals.
//!
//! Clef answers every question in one pass with real per-option probabilities, which is what
//! the escalation threshold and the reply-confidence judge are calibrated on.

use allama::DecisionProvider;
use allama::decision::{
    Answer as AllamaAnswer, ChoiceQuestion, DecisionInput, ScoreQuestion, YesNoQuestion,
};
use async_trait::async_trait;
use techdepo_ports::{
    Answer, AnswerValue, Classification, Classifier, ClassifierError, OptionProbability, Question,
    QuestionKind,
};

/// The levels a score question is asked on; the answer is rescaled to `0.0..=1.0`.
const SCORE_LEVELS: [&str; 5] = ["very low", "low", "medium", "high", "very high"];

/// A decision model (Clef) as a [`Classifier`].
#[derive(Debug, Clone)]
pub struct DecisionClassifier<P> {
    provider: P,
    model: String,
}

impl<P: DecisionProvider> DecisionClassifier<P> {
    /// `provider`, asking `model` (e.g. `@cf/cloudflare/clef`).
    pub fn new(provider: P, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }
}

#[async_trait]
impl<P> Classifier for DecisionClassifier<P>
where
    P: DecisionProvider + Send + Sync,
{
    async fn classify(
        &self,
        text: &str,
        questions: &[Question],
    ) -> Result<Classification, ClassifierError> {
        check_questions(questions)?;
        let mut request = allama::DecisionRequest::new(&self.model, DecisionInput::text(text));
        for question in questions {
            request = request.question(question.id.as_str(), to_allama(question));
        }
        let response = self
            .provider
            .decide(&request)
            .await
            .map_err(classifier_error)?;

        let answers = questions
            .iter()
            .map(|question| {
                let answer = response.answer(&question.id).ok_or_else(|| {
                    ClassifierError::InvalidResponse(format!("no answer to `{}`", question.id))
                })?;
                Ok(Answer {
                    question_id: question.id.clone(),
                    value: from_allama(question, answer)?,
                })
            })
            .collect::<Result<_, ClassifierError>>()?;
        Ok(Classification {
            model: response.model.clone().unwrap_or_else(|| self.model.clone()),
            answers,
            usage: response.usage.as_ref().map(crate::chat::usage),
        })
    }
}

/// Rejects what no model can answer: no questions, a choice without options, duplicate IDs.
pub(crate) fn check_questions(questions: &[Question]) -> Result<(), ClassifierError> {
    if questions.is_empty() {
        return Err(ClassifierError::InvalidRequest("no questions".into()));
    }
    for (i, question) in questions.iter().enumerate() {
        if questions[..i].iter().any(|q| q.id == question.id) {
            return Err(ClassifierError::InvalidRequest(format!(
                "duplicate question `{}`",
                question.id
            )));
        }
        if let QuestionKind::Choice { options } = &question.kind
            && options.is_empty()
        {
            return Err(ClassifierError::InvalidRequest(format!(
                "choice `{}` has no options",
                question.id
            )));
        }
    }
    Ok(())
}

fn to_allama(question: &Question) -> allama::decision::Question {
    match &question.kind {
        QuestionKind::Choice { options } => options
            .iter()
            .fold(ChoiceQuestion::new(&question.text), |q, o| {
                q.option(&o.id, &o.criteria)
            })
            .into(),
        QuestionKind::Noul => YesNoQuestion::new(&question.text).into(),
        QuestionKind::Score => ScoreQuestion::new(&question.text)
            .levels(SCORE_LEVELS)
            .into(),
    }
}

fn from_allama(question: &Question, answer: &AllamaAnswer) -> Result<AnswerValue, ClassifierError> {
    let mismatch = || {
        ClassifierError::InvalidResponse(format!("answer to `{}` has the wrong type", question.id))
    };
    match (&question.kind, answer) {
        (QuestionKind::Choice { options }, AllamaAnswer::Choice(choice)) => {
            let probabilities = options
                .iter()
                .map(|o| (o.id.clone(), choice.probability(&o.id).unwrap_or(0.0)))
                .collect();
            Ok(choice_value(probabilities, Some(&choice.chosen)))
        }
        (QuestionKind::Noul, AllamaAnswer::YesNo(yes_no)) => {
            let yes = yes_no.probability.clamp(0.0, 1.0);
            Ok(AnswerValue::Noul { yes, no: 1.0 - yes })
        }
        (QuestionKind::Score, AllamaAnswer::Score(score)) => {
            let levels = score.probabilities.len().max(SCORE_LEVELS.len());
            #[allow(clippy::cast_precision_loss)] // a handful of levels
            let top = (levels - 1) as f64;
            Ok(AnswerValue::Score {
                score: (score.score / top).clamp(0.0, 1.0),
            })
        }
        _ => Err(mismatch()),
    }
}

/// A choice answer from `(option, p)` pairs: normalized to sum to 1, most probable first, the
/// model's pick (when it is an option) or the top option as the choice.
pub(crate) fn choice_value(
    mut probabilities: Vec<(String, f64)>,
    chosen: Option<&str>,
) -> AnswerValue {
    let total: f64 = probabilities.iter().map(|(_, p)| p.max(0.0)).sum();
    for (_, p) in &mut probabilities {
        *p = if total > 0.0 { p.max(0.0) / total } else { 0.0 };
    }
    probabilities.sort_by(|a, b| b.1.total_cmp(&a.1));
    let choice = chosen
        .filter(|c| probabilities.iter().any(|(o, _)| o == c))
        .or_else(|| probabilities.first().map(|(o, _)| o.as_str()))
        .unwrap_or_default()
        .to_owned();
    AnswerValue::Choice {
        choice,
        probabilities: probabilities
            .into_iter()
            .map(|(option, p)| OptionProbability { option, p })
            .collect(),
    }
}

pub(crate) fn classifier_error(error: allama::Error) -> ClassifierError {
    if error.is_retryable() {
        return ClassifierError::Unavailable(error.to_string());
    }
    match error {
        allama::Error::InvalidRequest { .. } | allama::Error::Unsupported { .. } => {
            ClassifierError::InvalidRequest(error.to_string())
        }
        other => ClassifierError::InvalidResponse(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use allama::decision::{ChoiceAnswer, ScoreAnswer, YesNoAnswer};
    use allama::mock::MockDecisionProvider;
    use futures::executor::block_on;
    use techdepo_ports::ChoiceOption;

    use super::*;

    fn questions() -> Vec<Question> {
        vec![
            Question::choice(
                "intent",
                "What does the customer want?",
                vec![
                    ChoiceOption::new("cancel_order", "Cancel an order."),
                    ChoiceOption::new("order_status", "Where is the order."),
                    ChoiceOption::new("other", "Anything else."),
                ],
            ),
            Question::noul("wants_human", "Does the customer ask for a person?"),
            Question::score("anger", "How angry is the customer?"),
        ]
    }

    #[test]
    fn asks_clef_and_converts_the_answers() {
        let response = allama::DecisionResponse::new([
            (
                "intent",
                ChoiceAnswer::new(
                    "cancel_order",
                    [
                        ("cancel_order", 0.81),
                        ("order_status", 0.15),
                        ("other", 0.04),
                    ],
                )
                .into(),
            ),
            ("wants_human", YesNoAnswer::new(0.07).into()),
            (
                "anger",
                ScoreAnswer::new(1.0, [0.1, 0.8, 0.1, 0.0, 0.0]).into(),
            ),
        ])
        .model("@cf/cloudflare/clef");
        let provider = MockDecisionProvider::with_response(response);
        let classifier = DecisionClassifier::new(provider.clone(), "@cf/cloudflare/clef");

        let c = block_on(classifier.classify("Please cancel TD-1042", &questions())).unwrap();
        assert_eq!(c.model, "@cf/cloudflare/clef");
        assert_eq!(c.choice("intent"), Some("cancel_order"));
        let (top, runner_up) = c.answer("intent").unwrap().top_two().unwrap();
        assert!((top - 0.81).abs() < 1e-9 && (runner_up - 0.15).abs() < 1e-9);
        assert!((c.yes("wants_human").unwrap() - 0.07).abs() < 1e-9);
        assert!((c.score("anger").unwrap() - 0.25).abs() < 1e-9);

        let sent = &provider.requests()[0];
        assert_eq!(sent.questions.len(), 3);
        assert_eq!(sent.input.as_text(), Some("Please cancel TD-1042"));
    }

    #[test]
    fn rejects_bad_requests_and_answers() {
        let classifier = DecisionClassifier::new(MockDecisionProvider::empty(), "clef");
        assert!(matches!(
            block_on(classifier.classify("x", &[])),
            Err(ClassifierError::InvalidRequest(_))
        ));

        let wrong_type = allama::DecisionResponse::new([
            ("intent", YesNoAnswer::new(0.5).into()),
            ("wants_human", YesNoAnswer::new(0.5).into()),
            (
                "anger",
                ScoreAnswer::new(0.0, [1.0, 0.0, 0.0, 0.0, 0.0]).into(),
            ),
        ]);
        let classifier =
            DecisionClassifier::new(MockDecisionProvider::with_response(wrong_type), "clef");
        assert!(block_on(classifier.classify("x", &questions())).is_err());
    }
}
