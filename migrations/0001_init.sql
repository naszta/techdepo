-- TechDepó D1 schema, version 1.
--
-- Conventions (details in migrations/README.md):
--   * Every table is org-scoped: `org_id` leads every primary key and index. Entity tables use
--     `PRIMARY KEY (org_id, id)` and composite foreign keys, so a row can never reference
--     another org's row. `email_log.org_id` is the one nullable org_id (inbound mail rejected
--     before the org was known).
--   * Instants are INTEGER unix epoch milliseconds, UTC (`*_at`). Simulated instants (per-order
--     virtual clock) are `sim_*_at`. Calendar dates are TEXT `YYYY-MM-DD` (`*_on`, `eta`, `day`).
--   * Money is INTEGER cents EUR, VAT-inclusive (`*_cents`). Credit notes carry negative amounts.
--   * Booleans are INTEGER 0/1. JSON documents are TEXT checked with json_valid (`*_json`).
--   * Enum columns are TEXT with a CHECK listing the wire names. The lists mirror the Rust enums
--     (techdepo-domain and techdepo-api-types `vocab`); a test in techdepo-api-types keeps them
--     in sync. NULL passes a CHECK, so optional enum columns use the same form.
--   * Tables are STRICT. D1 enforces foreign keys.

-- ---------------------------------------------------------------------------------------------
-- Tenancy and identity
-- ---------------------------------------------------------------------------------------------

-- One Clerk organization = one webshop instance. Lazily provisioned by `Org/provision`.
CREATE TABLE orgs (
    id              TEXT    NOT NULL PRIMARY KEY,  -- Clerk org id (`org_…`), or `org_dev_…` in AUTH_MODE=dev
    public_id       TEXT    NOT NULL UNIQUE
                    CHECK (length(public_id) = 8 AND public_id NOT GLOB '*[^a-z0-9]*'),
    name            TEXT    NOT NULL,
    settings_json   TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(settings_json)),  -- domain::OrgSettings
    catalog_version TEXT,                          -- catalog version inventory was last seeded from
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
) STRICT;

-- Cache of org members. Written by the web worker on authenticated requests (identity cache,
-- not business state). In AUTH_MODE=dev it is the member directory mail-in verifies senders
-- against; in clerk mode Clerk is authoritative and this backs the admin Customers page.
CREATE TABLE org_members (
    org_id       TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id      TEXT    NOT NULL,
    email        TEXT    NOT NULL CHECK (email = lower(email)),
    name         TEXT,
    role         TEXT    NOT NULL CHECK (role IN ('org:admin', 'org:member')),
    source       TEXT    NOT NULL CHECK (source IN ('clerk', 'dev')),
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, user_id)
) STRICT;

CREATE UNIQUE INDEX org_members_by_email ON org_members (org_id, email);

-- ---------------------------------------------------------------------------------------------
-- Shop: stock, carts, builds, addresses
-- ---------------------------------------------------------------------------------------------

-- Per-org stock. Products themselves are compiled in (techdepo-catalog); no products table.
-- The CHECK makes an oversell abort the whole batch that tries it.
CREATE TABLE inventory (
    org_id     TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    product_id TEXT    NOT NULL,
    stock      INTEGER NOT NULL CHECK (stock >= 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, product_id)
) STRICT;

-- One server-side cart per user.
CREATE TABLE carts (
    org_id     TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id    TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, user_id)
) STRICT;

CREATE TABLE cart_items (
    org_id     TEXT    NOT NULL,
    user_id    TEXT    NOT NULL,
    product_id TEXT    NOT NULL,
    quantity   INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 99),
    added_at   INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (org_id, user_id, product_id),
    FOREIGN KEY (org_id, user_id) REFERENCES carts (org_id, user_id) ON DELETE CASCADE
) STRICT;

-- Saved PC builds. The builder's working draft lives in the browser; saving writes here.
CREATE TABLE builds (
    org_id           TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id               TEXT    NOT NULL,
    user_id          TEXT    NOT NULL,
    name             TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    product_ids_json TEXT    NOT NULL
                     CHECK (json_valid(product_ids_json) AND json_type(product_ids_json) = 'array'),
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (org_id, id)
) STRICT;

CREATE INDEX builds_by_user ON builds (org_id, user_id, updated_at DESC);

-- Customer address book (account profile).
CREATE TABLE customer_addresses (
    org_id             TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                 TEXT    NOT NULL,
    user_id            TEXT    NOT NULL,
    label              TEXT    NOT NULL CHECK (length(label) BETWEEN 1 AND 40),
    address_json       TEXT    NOT NULL CHECK (json_valid(address_json)),  -- api_types::Address
    is_default_shipping INTEGER NOT NULL DEFAULT 0 CHECK (is_default_shipping IN (0, 1)),
    is_default_billing INTEGER NOT NULL DEFAULT 0 CHECK (is_default_billing IN (0, 1)),
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    PRIMARY KEY (org_id, id)
) STRICT;

CREATE INDEX customer_addresses_by_user ON customer_addresses (org_id, user_id);

-- ---------------------------------------------------------------------------------------------
-- Orders (read model written by the `Order` virtual object)
-- ---------------------------------------------------------------------------------------------

-- Order number allocation (`TD-1001`, …): upsert + RETURNING in the same batch as the order
-- insert. Not reset by demo reset, so old emails never point at a new order.
CREATE TABLE order_sequences (
    org_id      TEXT    NOT NULL PRIMARY KEY REFERENCES orgs (id) ON DELETE CASCADE,
    last_number INTEGER NOT NULL CHECK (last_number > 1000)
) STRICT;

CREATE TABLE orders (
    org_id                TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                    TEXT    NOT NULL,
    number                INTEGER NOT NULL CHECK (number > 0),  -- displayed as TD-<number>
    user_id               TEXT    NOT NULL,
    customer_email        TEXT    NOT NULL,
    customer_name         TEXT,
    status                TEXT    NOT NULL CHECK (status IN ('awaiting_payment', 'paid', 'processing', 'shipped', 'delivered', 'cancelled', 'refunded', 'return_requested', 'returned')),
    -- Overlay flags (domain::OrderFlags).
    refund_pending        INTEGER NOT NULL DEFAULT 0 CHECK (refund_pending IN (0, 1)),
    investigation_open    INTEGER NOT NULL DEFAULT 0 CHECK (investigation_open IN (0, 1)),
    replacement_for       TEXT,                     -- id of the lost order this one replaces
    -- Totals, VAT-inclusive. Replacement orders are paid by the store (all prices 0).
    items_total_cents     INTEGER NOT NULL CHECK (items_total_cents >= 0),
    shipping_cents        INTEGER NOT NULL DEFAULT 0 CHECK (shipping_cents >= 0),
    total_cents           INTEGER NOT NULL CHECK (total_cents = items_total_cents + shipping_cents),
    vat_rate_bp           INTEGER NOT NULL CHECK (vat_rate_bp BETWEEN 0 AND 10000),
    -- 'store' only on replacement orders, which the store pays for (no payments rows).
    payment_method        TEXT    NOT NULL CHECK (payment_method IN ('card', 'bank_transfer', 'store')),
    shipping_address_json TEXT    NOT NULL CHECK (json_valid(shipping_address_json)),
    billing_address_json  TEXT    CHECK (json_valid(billing_address_json)),  -- NULL: same as shipping
    compat_override       INTEGER NOT NULL DEFAULT 0 CHECK (compat_override IN (0, 1)),
    -- Real instants of the milestones (simulated ones are in order_events.sim_at).
    paid_at               INTEGER,
    shipped_at            INTEGER,
    delivered_at          INTEGER,
    cancelled_at          INTEGER,
    refunded_at           INTEGER,
    returned_at           INTEGER,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, number),
    FOREIGN KEY (org_id, replacement_for) REFERENCES orders (org_id, id),
    CHECK ((payment_method = 'store') = (replacement_for IS NOT NULL))
) STRICT;

CREATE INDEX orders_by_created ON orders (org_id, created_at DESC);
CREATE INDEX orders_by_user ON orders (org_id, user_id, created_at DESC);
CREATE INDEX orders_by_status ON orders (org_id, status, created_at DESC);

-- Order lines snapshot the product as sold.
CREATE TABLE order_items (
    org_id           TEXT    NOT NULL,
    order_id         TEXT    NOT NULL,
    line_no          INTEGER NOT NULL CHECK (line_no >= 1),
    product_id       TEXT    NOT NULL,
    sku              TEXT    NOT NULL,
    name             TEXT    NOT NULL,
    category         TEXT    NOT NULL CHECK (category IN ('cpu', 'motherboard', 'memory', 'storage', 'gpu', 'case', 'psu', 'cooler')),
    unit_price_cents INTEGER NOT NULL CHECK (unit_price_cents >= 0),
    quantity         INTEGER NOT NULL CHECK (quantity >= 1),
    line_total_cents INTEGER NOT NULL CHECK (line_total_cents = unit_price_cents * quantity),
    PRIMARY KEY (org_id, order_id, line_no),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE
) STRICT;

-- Append-only timeline. Written only by the `Order` VO, which assigns `seq`, so inserts are
-- idempotent (`ON CONFLICT DO NOTHING`) when a ctx.run is retried.
CREATE TABLE order_events (
    org_id        TEXT    NOT NULL,
    order_id      TEXT    NOT NULL,
    seq           INTEGER NOT NULL CHECK (seq >= 1),
    kind          TEXT    NOT NULL CHECK (kind IN ('placed', 'payment_captured', 'payment_declined', 'processing_started', 'email_sent', 'invoice_issued', 'invoice_voided', 'credit_note_issued', 'shipped', 'carrier_update', 'delivered', 'cancelled', 'return_requested', 'returned', 'refund_requested', 'refund_approved', 'refund_rejected', 'refunded', 'investigation_opened', 'investigation_closed', 'replacement_created', 'ticket_linked', 'faults_changed', 'fault_triggered', 'clock_advanced', 'ship_date_changed', 'note')),
    actor         TEXT    NOT NULL CHECK (actor IN ('ai', 'human', 'system', 'customer')),
    actor_user_id TEXT,
    status_after  TEXT    CHECK (status_after IN ('awaiting_payment', 'paid', 'processing', 'shipped', 'delivered', 'cancelled', 'refunded', 'return_requested', 'returned')),
    summary       TEXT    NOT NULL,                -- one line for the timeline
    ref_kind      TEXT    CHECK (ref_kind IN ('order', 'payment', 'invoice', 'shipment', 'ticket', 'refund', 'investigation', 'human_task', 'email', 'agent_run')),
    ref_id        TEXT,
    data_json     TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(data_json)),
    invocation_id TEXT,                            -- Restate invocation, for UI deep links
    at            INTEGER NOT NULL,
    sim_at        INTEGER NOT NULL,                -- the order's simulated now
    PRIMARY KEY (org_id, order_id, seq),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE,
    CHECK ((ref_kind IS NULL) = (ref_id IS NULL))
) STRICT;

CREATE INDEX order_events_by_time ON order_events (org_id, at DESC);

-- Fake PSP ledger: charges and refunds. Amounts are positive; `kind` gives the direction.
CREATE TABLE payments (
    org_id         TEXT    NOT NULL,
    id             TEXT    NOT NULL,
    order_id       TEXT    NOT NULL,
    kind           TEXT    NOT NULL CHECK (kind IN ('charge', 'refund')),
    status         TEXT    NOT NULL CHECK (status IN ('succeeded', 'failed')),
    amount_cents   INTEGER NOT NULL CHECK (amount_cents > 0),
    method         TEXT    NOT NULL CHECK (method IN ('card', 'bank_transfer')),
    provider_ref   TEXT    NOT NULL,               -- fake PSP reference (`ch_…`, `re_…`)
    refund_of      TEXT,                           -- refunds: the charge being reversed
    refund_id      TEXT,                           -- refunds: the RefundWorkflow that issued it
    failure_reason TEXT,
    created_at     INTEGER NOT NULL,
    sim_at         INTEGER NOT NULL,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, refund_of) REFERENCES payments (org_id, id),
    CHECK ((kind = 'refund') = (refund_of IS NOT NULL)),
    CHECK ((status = 'failed') = (failure_reason IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX payments_by_provider_ref ON payments (org_id, provider_ref);
CREATE INDEX payments_by_order ON payments (org_id, order_id, created_at);
CREATE INDEX payments_by_time ON payments (org_id, created_at DESC);

-- ---------------------------------------------------------------------------------------------
-- Invoices (immutable documents; fixes are a credit note plus a reissued invoice)
-- ---------------------------------------------------------------------------------------------

-- Per org per year. Allocate with an upsert + RETURNING in the same batch as the invoice
-- insert (see README), so retries neither skip nor duplicate numbers.
CREATE TABLE invoice_sequences (
    org_id   TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    year     INTEGER NOT NULL CHECK (year BETWEEN 2000 AND 9999),
    last_seq INTEGER NOT NULL CHECK (last_seq >= 1),
    PRIMARY KEY (org_id, year)
) STRICT;

CREATE TABLE invoices (
    org_id               TEXT    NOT NULL,
    id                   TEXT    NOT NULL,
    order_id             TEXT    NOT NULL,
    number               TEXT    NOT NULL CHECK (number = printf('TD-%04d-%06d', year, seq)),
    year                 INTEGER NOT NULL,
    seq                  INTEGER NOT NULL CHECK (seq >= 1),
    kind                 TEXT    NOT NULL CHECK (kind IN ('invoice', 'credit_note')),
    status               TEXT    NOT NULL CHECK (status IN ('issued', 'voided')),
    supersedes_id        TEXT,                     -- reissued invoice: the voided one it replaces
    credit_for_id        TEXT,                     -- credit note: the invoice it cancels
    billing_name         TEXT    NOT NULL,
    billing_address_json TEXT    CHECK (json_valid(billing_address_json)),  -- NULL: the "missing address" fault
    gross_cents          INTEGER NOT NULL,
    net_cents            INTEGER NOT NULL,
    vat_cents            INTEGER NOT NULL,
    vat_rate_bp          INTEGER NOT NULL CHECK (vat_rate_bp BETWEEN 0 AND 10000),
    issue_date           TEXT    NOT NULL CHECK (issue_date IS date(issue_date)),  -- simulated date
    issued_at            INTEGER NOT NULL,
    voided_at            INTEGER,
    void_reason          TEXT,
    html                 TEXT    NOT NULL,         -- the document as issued
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, number),
    UNIQUE (org_id, year, seq),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, supersedes_id) REFERENCES invoices (org_id, id),
    FOREIGN KEY (org_id, credit_for_id) REFERENCES invoices (org_id, id),
    CHECK (gross_cents = net_cents + vat_cents),
    CHECK ((kind = 'credit_note') = (credit_for_id IS NOT NULL)),
    CHECK (kind = 'invoice' OR (status = 'issued' AND supersedes_id IS NULL AND gross_cents <= 0)),
    CHECK (kind = 'credit_note' OR gross_cents >= 0),
    CHECK ((status = 'voided') = (voided_at IS NOT NULL))
) STRICT;

CREATE INDEX invoices_by_order ON invoices (org_id, order_id, issued_at);
CREATE INDEX invoices_by_status ON invoices (org_id, status, issued_at DESC);
CREATE INDEX invoices_by_time ON invoices (org_id, issued_at DESC);

CREATE TABLE invoice_lines (
    org_id           TEXT    NOT NULL,
    invoice_id       TEXT    NOT NULL,
    line_no          INTEGER NOT NULL CHECK (line_no >= 1),
    product_id       TEXT,
    sku              TEXT,
    description      TEXT    NOT NULL,
    quantity         INTEGER NOT NULL CHECK (quantity >= 1),
    unit_price_cents INTEGER NOT NULL,             -- gross; negative on credit notes
    gross_cents      INTEGER NOT NULL,
    net_cents        INTEGER NOT NULL,
    vat_cents        INTEGER NOT NULL,
    PRIMARY KEY (org_id, invoice_id, line_no),
    FOREIGN KEY (org_id, invoice_id) REFERENCES invoices (org_id, id) ON DELETE CASCADE,
    CHECK (gross_cents = unit_price_cents * quantity),
    CHECK (gross_cents = net_cents + vat_cents)
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Fake carrier
-- ---------------------------------------------------------------------------------------------

-- `status` is what the customer sees; `lost_outcome` and `trace_status` are the hidden truth
-- (admin and demo controls only).
CREATE TABLE shipments (
    org_id           TEXT    NOT NULL,
    id               TEXT    NOT NULL,
    order_id         TEXT    NOT NULL,
    carrier          TEXT    NOT NULL,
    tracking_number  TEXT    NOT NULL,
    status           TEXT    NOT NULL CHECK (status IN ('in_transit', 'delivered')),
    lost_outcome     TEXT    CHECK (lost_outcome IN ('resurfaces', 'gone')),
    trace_status     TEXT    CHECK (trace_status IN ('open', 'resurfaced', 'no_information')),
    shipped_on       TEXT    NOT NULL CHECK (shipped_on IS date(shipped_on)),  -- simulated
    shipped_sim_at   INTEGER NOT NULL,
    eta              TEXT    NOT NULL CHECK (eta IS date(eta)),
    delivered_on     TEXT    CHECK (delivered_on IS date(delivered_on)),
    delivered_sim_at INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (org_id, id),
    UNIQUE (org_id, order_id),
    UNIQUE (org_id, tracking_number),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE,
    CHECK ((status = 'delivered') = (delivered_on IS NOT NULL))
) STRICT;

CREATE TABLE carrier_events (
    org_id      TEXT    NOT NULL,
    shipment_id TEXT    NOT NULL,
    seq         INTEGER NOT NULL CHECK (seq >= 1),
    kind        TEXT    NOT NULL CHECK (kind IN ('label_created', 'picked_up', 'in_transit', 'out_for_delivery', 'delivered', 'delayed', 'trace_opened', 'trace_found', 'trace_no_information')),
    internal    INTEGER NOT NULL DEFAULT 0 CHECK (internal IN (0, 1)),  -- 1: hidden from the customer
    description TEXT    NOT NULL,
    location    TEXT,
    at          INTEGER NOT NULL,
    sim_at      INTEGER NOT NULL,
    PRIMARY KEY (org_id, shipment_id, seq),
    FOREIGN KEY (org_id, shipment_id) REFERENCES shipments (org_id, id) ON DELETE CASCADE
) STRICT;

-- ---------------------------------------------------------------------------------------------
-- Support
-- ---------------------------------------------------------------------------------------------

-- Read model of the `SupportTicket` VO. `id` is the display form (`T-7F2M9Q`).
CREATE TABLE tickets (
    org_id              TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                  TEXT    NOT NULL
                        CHECK (id GLOB 'T-*' AND length(id) BETWEEN 6 AND 14
                               AND substr(id, 3) NOT GLOB '*[^0-9A-HJKMNP-TV-Z]*'),
    user_id             TEXT    NOT NULL,
    customer_email      TEXT    NOT NULL,
    subject             TEXT    NOT NULL,
    channel             TEXT    NOT NULL CHECK (channel IN ('email', 'web')),  -- of the first message
    status              TEXT    NOT NULL CHECK (status IN ('new', 'routing', 'with_route', 'awaiting_customer', 'with_human', 'resolved')),
    assignee            TEXT    NOT NULL DEFAULT 'ai' CHECK (assignee IN ('ai', 'human')),
    order_id            TEXT,
    order_number        INTEGER,                   -- as extracted from the message, even if unknown
    intent              TEXT    CHECK (intent IN ('cancel_order', 'order_status', 'billing_problem', 'refund_or_return', 'product_advice', 'order_question', 'other')),
    intent_confidence   REAL    CHECK (intent_confidence BETWEEN 0 AND 1),
    classification_json TEXT    CHECK (json_valid(classification_json)),  -- api_types Classification
    route               TEXT    CHECK (route IN ('cancel_order', 'where_is_my_order', 'billing_issue', 'refund_request', 'product_advisor', 'order_assistant', 'human')),
    awaiting            TEXT    CHECK (awaiting IN ('order_number', 'offer_reply', 'clarification')),
    escalation_reason   TEXT,
    reply_confidence    REAL    CHECK (reply_confidence BETWEEN 0 AND 1),
    message_count       INTEGER NOT NULL DEFAULT 0 CHECK (message_count >= 0),
    last_message_at     INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    resolved_at         INTEGER,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id),
    CHECK ((status = 'awaiting_customer') = (awaiting IS NOT NULL))
) STRICT;

CREATE INDEX tickets_by_status ON tickets (org_id, status, updated_at DESC);
CREATE INDEX tickets_by_user ON tickets (org_id, user_id, updated_at DESC);
CREATE INDEX tickets_by_order ON tickets (org_id, order_id);
CREATE INDEX tickets_by_updated ON tickets (org_id, updated_at DESC);

-- The conversation. `seq` is assigned by the `SupportTicket` VO (idempotent inserts).
-- Outbound AI drafts awaiting review are `draft`; customers only ever see `received`/`sent`.
CREATE TABLE ticket_messages (
    org_id         TEXT    NOT NULL,
    ticket_id      TEXT    NOT NULL,
    seq            INTEGER NOT NULL CHECK (seq >= 1),
    direction      TEXT    NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    author         TEXT    NOT NULL CHECK (author IN ('ai', 'human', 'system', 'customer')),
    author_user_id TEXT,
    channel        TEXT    NOT NULL CHECK (channel IN ('email', 'web')),
    status         TEXT    NOT NULL CHECK (status IN ('received', 'draft', 'sent', 'discarded')),
    subject        TEXT    NOT NULL,
    body_text      TEXT    NOT NULL,
    body_html      TEXT,
    message_id     TEXT,                           -- RFC 5322 Message-ID, without angle brackets
    email_log_id   TEXT,
    agent_run_id   TEXT,
    created_at     INTEGER NOT NULL,
    sent_at        INTEGER,
    PRIMARY KEY (org_id, ticket_id, seq),
    FOREIGN KEY (org_id, ticket_id) REFERENCES tickets (org_id, id) ON DELETE CASCADE,
    CHECK ((direction = 'inbound') = (author = 'customer')),
    CHECK ((direction = 'inbound') = (status = 'received'))
) STRICT;

CREATE UNIQUE INDEX ticket_messages_by_message_id ON ticket_messages (org_id, message_id)
    WHERE message_id IS NOT NULL;

-- Mirrors the `Investigation` workflow.
CREATE TABLE investigations (
    org_id               TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                   TEXT    NOT NULL,
    order_id             TEXT    NOT NULL,
    ticket_id            TEXT,                     -- NULL when an admin opened it from an order
    shipment_id          TEXT,
    status               TEXT    NOT NULL CHECK (status IN ('open', 'tracing', 'awaiting_decision', 'awaiting_customer', 'resolved', 'escalated')),
    carrier_result       TEXT    CHECK (carrier_result IN ('resurfaces', 'gone')),
    decision             TEXT    CHECK (decision IN ('offer_refund', 'offer_reship', 'offer_both', 'custom_reply')),
    outcome              TEXT    CHECK (outcome IN ('resurfaced', 'refunded', 'reshipped', 'declined', 'escalated', 'custom_reply')),
    rounds               INTEGER NOT NULL DEFAULT 0 CHECK (rounds BETWEEN 0 AND 2),
    opened_by            TEXT    NOT NULL CHECK (opened_by IN ('ai', 'human', 'system', 'customer')),
    opened_by_user_id    TEXT,
    note                 TEXT,
    human_task_id        TEXT,
    refund_id            TEXT,
    replacement_order_id TEXT,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    closed_at            INTEGER,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id),
    FOREIGN KEY (org_id, ticket_id) REFERENCES tickets (org_id, id),
    FOREIGN KEY (org_id, shipment_id) REFERENCES shipments (org_id, id),
    FOREIGN KEY (org_id, replacement_order_id) REFERENCES orders (org_id, id),
    CHECK ((status IN ('resolved', 'escalated')) = (closed_at IS NOT NULL))
) STRICT;

CREATE INDEX investigations_by_status ON investigations (org_id, status, created_at DESC);
CREATE INDEX investigations_by_order ON investigations (org_id, order_id);
CREATE INDEX investigations_by_ticket ON investigations (org_id, ticket_id);

-- Mirrors `RefundWorkflow`. Only that workflow moves money.
CREATE TABLE refunds (
    org_id            TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                TEXT    NOT NULL,
    order_id          TEXT    NOT NULL,
    ticket_id         TEXT,
    investigation_id  TEXT,
    amount_cents      INTEGER NOT NULL CHECK (amount_cents > 0),
    reason            TEXT    NOT NULL CHECK (reason IN ('cancellation', 'duplicate_charge', 'return', 'lost_parcel', 'goodwill', 'other')),
    reason_text       TEXT,
    status            TEXT    NOT NULL CHECK (status IN ('requested', 'approved', 'rejected', 'completed')),
    pre_approved      INTEGER NOT NULL DEFAULT 0 CHECK (pre_approved IN (0, 1)),
    charge_payment_id TEXT,                        -- the charge to reverse (the duplicate, for double charges)
    refund_payment_id TEXT,
    credit_note_id    TEXT,
    human_task_id     TEXT,
    decided_by        TEXT,                        -- user id of the approver/rejecter
    decided_at        INTEGER,
    rejection_reason  TEXT,
    evidence_json     TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),  -- [api_types::Evidence]
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    completed_at      INTEGER,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id),
    FOREIGN KEY (org_id, ticket_id) REFERENCES tickets (org_id, id),
    FOREIGN KEY (org_id, investigation_id) REFERENCES investigations (org_id, id),
    FOREIGN KEY (org_id, charge_payment_id) REFERENCES payments (org_id, id),
    FOREIGN KEY (org_id, refund_payment_id) REFERENCES payments (org_id, id),
    FOREIGN KEY (org_id, credit_note_id) REFERENCES invoices (org_id, id),
    CHECK ((status = 'completed') = (completed_at IS NOT NULL)),
    CHECK ((status = 'rejected') = (rejection_reason IS NOT NULL))
) STRICT;

CREATE INDEX refunds_by_status ON refunds (org_id, status, created_at DESC);
CREATE INDEX refunds_by_order ON refunds (org_id, order_id);
CREATE INDEX refunds_by_ticket ON refunds (org_id, ticket_id);

-- One queue for every decision a person has to make. `awakeable_id` never leaves the server:
-- only `HumanTasks/decide` resolves it. Only the kinds a workflow waits on have one
-- (`HumanTaskKind::has_waiter`); decisions on the ticket-owned kinds go to the ticket
-- (`SupportTicket/task_decided`).
CREATE TABLE human_tasks (
    org_id                 TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id                     TEXT    NOT NULL,
    kind                   TEXT    NOT NULL CHECK (kind IN ('approve_refund', 'investigation_decision', 'review_reply', 'escalation', 'offers_declined')),
    status                 TEXT    NOT NULL CHECK (status IN ('pending', 'decided', 'cancelled')),
    title                  TEXT    NOT NULL,
    summary                TEXT    NOT NULL,
    context_json           TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(context_json)),  -- api_types TaskContext
    evidence_json          TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),  -- [Evidence]
    allowed_decisions_json TEXT    NOT NULL
                           CHECK (json_valid(allowed_decisions_json) AND json_type(allowed_decisions_json) = 'array'),
    awakeable_id           TEXT,
    decision               TEXT    CHECK (decision IN ('approve', 'reject', 'offer_refund', 'offer_reship', 'offer_both', 'custom_reply', 'send', 'edit_and_send', 'discard', 'reply', 'hand_back', 'close')),
    decision_json          TEXT    CHECK (json_valid(decision_json)),  -- api_types HumanDecision
    note                   TEXT,
    decided_by             TEXT,
    decided_at             INTEGER,
    sla_due_at             INTEGER NOT NULL,
    nudged_at              INTEGER,
    ticket_id              TEXT,
    order_id               TEXT,
    refund_id              TEXT,
    investigation_id       TEXT,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, ticket_id) REFERENCES tickets (org_id, id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id),
    CHECK ((status = 'decided') = (decision IS NOT NULL)),
    CHECK ((decision IS NULL) = (decided_at IS NULL)),
    CHECK ((awakeable_id IS NOT NULL) = (kind IN ('approve_refund', 'investigation_decision')))
) STRICT;

CREATE INDEX human_tasks_by_status ON human_tasks (org_id, status, created_at DESC);
CREATE INDEX human_tasks_by_ticket ON human_tasks (org_id, ticket_id);
CREATE INDEX human_tasks_by_order ON human_tasks (org_id, order_id);

-- Every inbound email (including rejected ones, with the reason) and every outbound copy.
-- `org_id` is NULL only for inbound mail rejected before the org was identified.
CREATE TABLE email_log (
    org_id          TEXT    REFERENCES orgs (id) ON DELETE CASCADE,
    id              TEXT    NOT NULL PRIMARY KEY,
    direction       TEXT    NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    channel         TEXT    NOT NULL DEFAULT 'email' CHECK (channel IN ('email', 'web')),
    status          TEXT    NOT NULL CHECK (status IN ('accepted', 'rejected', 'queued', 'sent', 'failed')),
    reject_reason   TEXT    CHECK (reject_reason IN ('auto_reply', 'own_domain', 'dmarc_fail', 'oversized', 'malformed', 'unknown_org', 'not_member', 'rate_limited', 'duplicate')),
    message_id      TEXT,                          -- without angle brackets
    in_reply_to     TEXT,
    references_json TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(references_json)),
    from_address    TEXT    NOT NULL,
    from_name       TEXT,
    to_address      TEXT    NOT NULL,
    reply_to        TEXT,
    subject         TEXT    NOT NULL DEFAULT '',
    text_body       TEXT,
    html_body       TEXT,
    raw_size        INTEGER CHECK (raw_size >= 0),
    auth_results    TEXT,                          -- inbound: Authentication-Results summary
    template        TEXT,                          -- outbound: template name
    provider_id     TEXT,                          -- outbound: Resend / Mailpit id
    error           TEXT,
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    user_id         TEXT,
    ticket_id       TEXT,
    order_id        TEXT,
    created_at      INTEGER NOT NULL,
    sent_at         INTEGER,
    CHECK ((direction = 'inbound' AND status IN ('accepted', 'rejected'))
        OR (direction = 'outbound' AND status IN ('queued', 'sent', 'failed'))),
    CHECK ((status = 'rejected') = (reject_reason IS NOT NULL)),
    CHECK (org_id IS NOT NULL OR status = 'rejected')
) STRICT;

CREATE INDEX email_log_by_time ON email_log (org_id, created_at DESC);
CREATE INDEX email_log_by_ticket ON email_log (org_id, ticket_id, created_at);
-- Dedupe on Message-ID among the mail we kept. A redelivered message is still logged, as
-- `rejected` with `reject_reason = 'duplicate'`, so rejected rows are outside the index.
CREATE UNIQUE INDEX email_log_by_message_id ON email_log (org_id, direction, message_id)
    WHERE org_id IS NOT NULL AND message_id IS NOT NULL AND status <> 'rejected';

-- ---------------------------------------------------------------------------------------------
-- AI traces and budgets
-- ---------------------------------------------------------------------------------------------

-- One run per LLM-backed decision: classification, an agent conversation turn, a judge pass,
-- or interpreting a customer's reply.
CREATE TABLE agent_runs (
    org_id           TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id               TEXT    NOT NULL,
    ticket_id        TEXT,
    investigation_id TEXT,
    agent            TEXT    NOT NULL CHECK (agent IN ('router', 'product_advisor', 'order_assistant', 'judge', 'reply_interpreter')),
    model            TEXT    NOT NULL,
    status           TEXT    NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
    outcome          TEXT    CHECK (outcome IN ('reply', 'ask_customer', 'escalate', 'reroute')),
    outcome_json     TEXT    CHECK (json_valid(outcome_json)),
    system_prompt    TEXT,
    input_tokens     INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens    INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    step_count       INTEGER NOT NULL DEFAULT 0 CHECK (step_count >= 0),
    attempt          INTEGER NOT NULL DEFAULT 1 CHECK (attempt >= 1),  -- 2 = guardrail retry
    reply_confidence REAL    CHECK (reply_confidence BETWEEN 0 AND 1),
    confidence_json  TEXT    CHECK (json_valid(confidence_json)),   -- api_types ReplyConfidence
    guardrails_json  TEXT    CHECK (json_valid(guardrails_json)),   -- [GuardrailResult]
    error            TEXT,
    started_at       INTEGER NOT NULL,
    finished_at      INTEGER,
    PRIMARY KEY (org_id, id),
    FOREIGN KEY (org_id, ticket_id) REFERENCES tickets (org_id, id) ON DELETE CASCADE,
    CHECK ((status = 'running') = (finished_at IS NULL))
) STRICT;

CREATE INDEX agent_runs_by_ticket ON agent_runs (org_id, ticket_id, started_at);
CREATE INDEX agent_runs_by_time ON agent_runs (org_id, started_at DESC);

CREATE TABLE agent_steps (
    org_id        TEXT    NOT NULL,
    run_id        TEXT    NOT NULL,
    idx           INTEGER NOT NULL CHECK (idx >= 0),
    kind          TEXT    NOT NULL CHECK (kind IN ('llm_call', 'tool_call', 'classification', 'guardrail')),
    name          TEXT    NOT NULL,                -- model, tool or guardrail name
    journal_name  TEXT,                            -- ctx.run name, e.g. `llm-0`
    input_json    TEXT    CHECK (json_valid(input_json)),
    output_json   TEXT    CHECK (json_valid(output_json)),
    is_error      INTEGER NOT NULL DEFAULT 0 CHECK (is_error IN (0, 1)),
    input_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    started_at    INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    PRIMARY KEY (org_id, run_id, idx),
    FOREIGN KEY (org_id, run_id) REFERENCES agent_runs (org_id, id) ON DELETE CASCADE
) STRICT;

-- Daily token accounting per org and model (budget enforcement). Upsert-increment.
CREATE TABLE llm_usage (
    org_id        TEXT    NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    day           TEXT    NOT NULL CHECK (day IS date(day)),  -- UTC
    model         TEXT    NOT NULL,
    calls         INTEGER NOT NULL DEFAULT 0 CHECK (calls >= 0),
    input_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    PRIMARY KEY (org_id, day, model)
) STRICT;

CREATE INDEX llm_usage_by_day ON llm_usage (day);

-- ---------------------------------------------------------------------------------------------
-- Demo simulation state (kept apart from the shop's business tables)
-- ---------------------------------------------------------------------------------------------

-- Fault toggles armed for an order's next steps (domain::FaultToggles, seeded from settings).
CREATE TABLE demo_faults (
    org_id             TEXT    NOT NULL,
    order_id           TEXT    NOT NULL,
    faults_json        TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(faults_json)),
    email_outage_until INTEGER,                    -- real instant until which email sends fail
    updated_at         INTEGER NOT NULL,
    PRIMARY KEY (org_id, order_id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE
) STRICT;

-- Per-order virtual clock: simulated now = real now + offset (domain::VirtualClock).
CREATE TABLE order_clock (
    org_id      TEXT    NOT NULL,
    order_id    TEXT    NOT NULL,
    offset_secs INTEGER NOT NULL DEFAULT 0 CHECK (offset_secs >= 0),
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (org_id, order_id),
    FOREIGN KEY (org_id, order_id) REFERENCES orders (org_id, id) ON DELETE CASCADE
) STRICT;
