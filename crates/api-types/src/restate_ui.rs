//! Deep links from the admin pages into the Restate UI ([`crate::session::Config::restate_ui_url`],
//! local development only).
//!
//! A timeline event links the invocation that wrote it. A record mirrored from a keyed Restate
//! component links that component's page instead, which lists its invocations and state: a
//! ticket its `SupportTicket` object, a refund or investigation its workflow, a human task the
//! component waiting for the decision.

use techdepo_domain::{InvestigationId, OrderId, OrgId, RefundId, TicketId};

use crate::ingress::{
    Handler, InvestigationRun, OrderDemoAction, RefundWorkflowRun, SupportTicketInbound, keys,
};
use crate::tasks::{HumanTaskDetail, TaskContext};

/// A page of the Restate UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestateUiLink {
    /// One invocation (`/ui/invocations/<id>`).
    Invocation(String),
    /// One virtual object instance: its state and invocations.
    Object {
        /// Object name.
        service: &'static str,
        /// Key.
        key: String,
    },
    /// One workflow run: its `run` invocation, state and interactions.
    Workflow {
        /// Workflow name.
        service: &'static str,
        /// Workflow ID.
        key: String,
    },
}

impl RestateUiLink {
    /// An invocation by ID.
    pub fn invocation(id: impl Into<String>) -> Self {
        Self::Invocation(id.into())
    }

    /// The `Order` object.
    pub fn order(order: &OrderId) -> Self {
        Self::Object {
            service: OrderDemoAction::INFO.service,
            key: keys::order(order),
        }
    }

    /// The `SupportTicket` object.
    pub fn ticket(org: &OrgId, ticket: &TicketId) -> Self {
        Self::Object {
            service: SupportTicketInbound::INFO.service,
            key: keys::ticket(org, ticket),
        }
    }

    /// The `RefundWorkflow` run.
    pub fn refund(refund: &RefundId) -> Self {
        Self::Workflow {
            service: RefundWorkflowRun::INFO.service,
            key: keys::refund(refund),
        }
    }

    /// The `Investigation` run.
    pub fn investigation(investigation: &InvestigationId) -> Self {
        Self::Workflow {
            service: InvestigationRun::INFO.service,
            key: keys::investigation(investigation),
        }
    }

    /// The component waiting for a human task's decision: the refund or investigation
    /// workflow, or the ticket for ticket-owned kinds (`None` without a ticket).
    pub fn task_owner(org: &OrgId, task: &HumanTaskDetail) -> Option<Self> {
        match &task.context {
            TaskContext::ApproveRefund { refund_id, .. } => Some(Self::refund(refund_id)),
            TaskContext::InvestigationDecision {
                investigation_id, ..
            }
            | TaskContext::OffersDeclined {
                investigation_id, ..
            } => Some(Self::investigation(investigation_id)),
            TaskContext::ReviewReply { .. } | TaskContext::Escalation { .. } => task
                .ticket_id
                .as_ref()
                .or(task.task.ticket.as_ref().map(|t| &t.id))
                .map(|ticket| Self::ticket(org, ticket)),
        }
    }

    /// The page's URL under `base` (the Restate admin URL, e.g. `http://localhost:9070`).
    pub fn url(&self, base: &str) -> String {
        let base = base.trim_end_matches('/');
        match self {
            Self::Invocation(id) => format!("{base}/ui/invocations/{}", segment(id)),
            Self::Object { service, key } => {
                format!("{base}/ui/virtual-objects/{service}/{}", segment(key))
            }
            Self::Workflow { service, key } => {
                format!("{base}/ui/workflows/{service}/{}", segment(key))
            }
        }
    }
}

/// `value` as one percent-encoded path segment.
fn segment(value: &str) -> String {
    let mut out = String::new();
    crate::ingress::encode_key(&mut out, value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let base = "http://localhost:9070/";
        assert_eq!(
            RestateUiLink::invocation("inv_1abc").url(base),
            "http://localhost:9070/ui/invocations/inv_1abc"
        );
        assert_eq!(
            RestateUiLink::ticket(&OrgId::new("org_1"), &TicketId::parse("T-7F2M9Q").unwrap())
                .url(base),
            "http://localhost:9070/ui/virtual-objects/SupportTicket/org_1:T-7F2M9Q"
        );
        assert_eq!(
            RestateUiLink::refund(&RefundId::new("ref_0123")).url(base),
            "http://localhost:9070/ui/workflows/RefundWorkflow/ref_0123"
        );
        assert_eq!(
            RestateUiLink::investigation(&InvestigationId::new("inv_9")).url(base),
            "http://localhost:9070/ui/workflows/Investigation/inv_9"
        );
        assert_eq!(
            RestateUiLink::order(&OrderId::new("ord a/b")).url(base),
            "http://localhost:9070/ui/virtual-objects/Order/ord%20a%2Fb"
        );
    }
}
