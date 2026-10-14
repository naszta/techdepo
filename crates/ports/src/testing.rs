//! In-memory test doubles for native tests (`features = ["testing"]`).
//!
//! The deterministic keyword-based LLM and classifier for local dev live in `crates/llm`
//! (`mock`); these are smaller, scriptable doubles for unit tests of services and the agent
//! loop.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use techdepo_domain::OrgId;

use crate::directory::{DirectoryError, Member, MemberDirectory};
use crate::llm::{ChatRequest, ChatResponse, Llm, LlmError};
use crate::mailer::{MailError, Mailer, OutboundEmail};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records every email and returns `test-<n>` provider IDs. Can be told to fail.
#[derive(Debug, Default)]
pub struct RecordingMailer {
    sent: Mutex<Vec<OutboundEmail>>,
    failures: Mutex<VecDeque<MailError>>,
}

impl RecordingMailer {
    /// An empty mailer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next send fails with `error` (queue several to fail several times).
    pub fn fail_next(&self, error: MailError) {
        lock(&self.failures).push_back(error);
    }

    /// Every email sent so far.
    pub fn sent(&self) -> Vec<OutboundEmail> {
        lock(&self.sent).clone()
    }
}

#[async_trait]
impl Mailer for RecordingMailer {
    async fn send(&self, email: &OutboundEmail) -> Result<String, MailError> {
        if let Some(error) = lock(&self.failures).pop_front() {
            return Err(error);
        }
        let mut sent = lock(&self.sent);
        sent.push(email.clone());
        Ok(format!("test-{}", sent.len()))
    }
}

/// A fixed member list.
#[derive(Debug, Default)]
pub struct StaticDirectory {
    members: Vec<(OrgId, Member)>,
}

impl StaticDirectory {
    /// An empty directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `member` to `org`.
    #[must_use]
    pub fn with(mut self, org: OrgId, member: Member) -> Self {
        self.members.push((org, member));
        self
    }
}

#[async_trait]
impl MemberDirectory for StaticDirectory {
    async fn is_member(&self, org: &OrgId, email: &str) -> Result<Option<Member>, DirectoryError> {
        Ok(self
            .members
            .iter()
            .find(|(o, m)| o == org && m.email.eq_ignore_ascii_case(email))
            .map(|(_, m)| m.clone()))
    }

    async fn list_members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
        Ok(self
            .members
            .iter()
            .filter(|(o, _)| o == org)
            .map(|(_, m)| m.clone())
            .collect())
    }
}

/// Answers each `chat` with the next scripted response and records the requests.
#[derive(Debug, Default)]
pub struct ScriptedLlm {
    responses: Mutex<VecDeque<Result<ChatResponse, LlmError>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    /// Model name reported by [`Llm::default_model`].
    pub const MODEL: &'static str = "scripted";

    /// A model that answers with `responses`, in order.
    pub fn new(responses: impl IntoIterator<Item = Result<ChatResponse, LlmError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::default(),
        }
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<ChatRequest> {
        lock(&self.requests).clone()
    }
}

#[async_trait]
impl Llm for ScriptedLlm {
    fn default_model(&self) -> &str {
        Self::MODEL
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        lock(&self.requests).push(request.clone());
        lock(&self.responses)
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::InvalidRequest("script exhausted".into())))
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use crate::directory::MemberRole;
    use crate::llm::{FinishReason, Usage};
    use crate::mailer::Mailbox;

    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn doubles_behave() {
        let mailer = RecordingMailer::new();
        let email = OutboundEmail {
            message_id: "x@techdepo.dev".into(),
            from: Mailbox::new("support@techdepo.dev"),
            to: Mailbox::new("a@b.c"),
            reply_to: None,
            subject: "Hi".into(),
            text: "Hello".into(),
            html: None,
            headers: vec![],
            attachments: vec![],
        };
        mailer.fail_next(MailError::Unavailable("down".into()));
        let first = mailer.send(&email);
        assert_send(&first);
        assert!(block_on(first).unwrap_err().is_retryable());
        assert_eq!(block_on(mailer.send(&email)).unwrap(), "test-1");
        assert_eq!(mailer.sent().len(), 1);

        let org = OrgId::new("org_1");
        let dir = StaticDirectory::new().with(
            org.clone(),
            Member {
                user_id: "user_1".into(),
                email: "jane@example.com".into(),
                name: None,
                role: MemberRole::Admin,
            },
        );
        assert!(
            block_on(dir.is_member(&org, "Jane@Example.com"))
                .unwrap()
                .is_some()
        );
        assert!(
            block_on(dir.is_member(&OrgId::new("org_2"), "jane@example.com"))
                .unwrap()
                .is_none()
        );

        let llm = ScriptedLlm::new([Ok(ChatResponse {
            model: "m".into(),
            text: Some("hi".into()),
            tool_calls: vec![],
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
        })]);
        let req = ChatRequest::default();
        assert_eq!(
            block_on(llm.chat(&req)).unwrap().text.as_deref(),
            Some("hi")
        );
        assert!(block_on(llm.chat(&req)).is_err());
        assert_eq!(llm.requests().len(), 2);
    }
}
