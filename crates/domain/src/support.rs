//! Support vocabulary: intents, ticket status, actors.

labeled_enum! {
    /// What a customer's message is about. The closed set the classifier chooses from.
    pub enum Intent {
        CancelOrder => ("cancel_order", "Cancel order"),
        OrderStatus => ("order_status", "Order status"),
        BillingProblem => ("billing_problem", "Billing problem"),
        RefundOrReturn => ("refund_or_return", "Refund or return"),
        ProductAdvice => ("product_advice", "Product advice"),
        OrderQuestion => ("order_question", "Order question"),
        Other => ("other", "Other"),
    }
}

impl Intent {
    /// One-line description, used in the classifier prompt and admin UI.
    pub const fn description(self) -> &'static str {
        match self {
            Self::CancelOrder => "The customer wants to cancel an order.",
            Self::OrderStatus => {
                "The customer asks where an order is, when it arrives, or reports it has not arrived."
            }
            Self::BillingProblem => {
                "A problem with a charge or invoice: double charge, wrong amount, missing or wrong invoice details."
            }
            Self::RefundOrReturn => "The customer wants their money back or wants to return items.",
            Self::ProductAdvice => {
                "Questions about products, specs, compatibility or recommendations before buying."
            }
            Self::OrderQuestion => {
                "Other questions about an existing order: items, addresses, delivery details, invoices to view."
            }
            Self::Other => "Anything else, or the message does not fit a single category above.",
        }
    }
}

labeled_enum! {
    /// Coarse ticket status, as stored in D1 and shown in the UI.
    pub enum TicketStatus {
        /// Just created; nothing has run yet.
        New => ("new", "New"),
        /// Being classified by the router.
        Routing => ("routing", "Routing"),
        /// A workflow or agent is handling it.
        WithRoute => ("with_route", "In progress"),
        /// Waiting for the customer to answer a question.
        AwaitingCustomer => ("awaiting_customer", "Awaiting customer"),
        /// A person owns it.
        WithHuman => ("with_human", "With a person"),
        /// Closed.
        Resolved => ("resolved", "Resolved"),
    }
}

impl TicketStatus {
    /// Not yet resolved.
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Resolved)
    }
}

labeled_enum! {
    /// Who did something (timeline entries, messages, decisions).
    pub enum Actor {
        Ai => ("ai", "AI"),
        Human => ("human", "Staff"),
        System => ("system", "System"),
        Customer => ("customer", "Customer"),
    }
}

labeled_enum! {
    /// Who currently handles a ticket.
    pub enum Assignee {
        Ai => ("ai", "AI"),
        Human => ("human", "Staff"),
    }
}

labeled_enum! {
    /// How a customer message reached us.
    pub enum Channel {
        Email => ("email", "Email"),
        Web => ("web", "Web form"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_round_trip() {
        assert_eq!(Intent::ALL.len(), 7);
        for intent in Intent::ALL {
            assert_eq!(intent.as_str().parse::<Intent>().unwrap(), *intent);
            assert!(!intent.description().is_empty());
        }
        assert_eq!(
            serde_json::to_string(&Intent::CancelOrder).unwrap(),
            "\"cancel_order\""
        );
        assert!("cancel".parse::<Intent>().is_err());
    }
}
