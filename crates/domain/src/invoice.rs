//! Invoice document kinds and states. Numbering lives in [`crate::id::InvoiceNumber`].

labeled_enum! {
    /// Invoices are immutable; a fix is a credit note plus a new invoice.
    pub enum InvoiceKind {
        Invoice => ("invoice", "Invoice"),
        /// Storno / credit note referencing the original invoice.
        CreditNote => ("credit_note", "Credit note"),
    }
}

labeled_enum! {
    /// Lifecycle of an invoice document.
    pub enum InvoiceStatus {
        Issued => ("issued", "Issued"),
        /// Cancelled by a credit note; superseded by a reissued invoice.
        Voided => ("voided", "Voided"),
    }
}
