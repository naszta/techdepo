# D1 migrations

One D1 (SQLite) database, `techdepo`, shared by all three workers. Migrations are plain SQL
applied by wrangler (`migrations_dir = "../../migrations"` in each `wrangler.toml`):

```bash
wrangler d1 migrations apply techdepo --local --persist-to ../../.wrangler/state   # local
wrangler d1 migrations apply techdepo --remote                                     # CI deploy
```

Never edit an applied migration. Add `NNNN_<what>.sql` instead. SQLite cannot alter a CHECK
constraint, so adding an enum value means rebuilding the table: create `<table>_new`, copy the
rows, drop the old table, rename, and recreate the indexes. Keep the SQL to the subset D1 supports
(SQLite 3.4x): no `PRAGMA` statements, no `ATTACH`, no triggers that write other tables.

Writes go through Restate handlers (`crates/services`, via `crates/db` repositories). The web
worker and mail-in only read business state. The exceptions are `org_members`, an identity cache
the web worker upserts, and the inbound rows of `email_log`, which mail-in writes (accepted, or
rejected with the reason) before it hands a message to Restate.

## Conventions

| Topic | Rule |
|---|---|
| Tenancy | Every table has `org_id TEXT`, and it leads every primary key and index, except `app_settings` (`0005`), which holds the operator's app-wide settings. Entity tables use `PRIMARY KEY (org_id, id)`; child tables use composite foreign keys `(org_id, parent_id)`, so a row can never reference another org's row. Every query filters on `org_id` (the `db` crate's `OrgScope`). The only exception is `email_log.org_id`, which is `NULL` for inbound mail rejected before the org was identified (`status = 'rejected'` is enforced). Those rows are visible in no org's admin. |
| `orgs` | Keyed by `id`, which is the Clerk org ID (`org_…`), or `org_dev_…` in `AUTH_MODE=dev`. `public_id` is the 8-character `[a-z0-9]` email tag. |
| IDs | `TEXT`. Opaque IDs are minted by the service that creates the row, from Restate's deterministic randomness (`ctx.rand_uuid()`), with a prefix: `ord_`, `pay_`, `inv_`, `shp_`, `ref_`, `inq_` (investigation), `tsk_` (human task), `run_`, `bld_`, `adr_`, `eml_`. Formatted IDs are stored in display form (`T-7F2M9Q`, `TD-2026-000042`), except order numbers, which are stored as `INTEGER` (`1042` for `TD-1042`). Catalog product IDs are the catalog slugs. |
| Instants | `INTEGER` **unix epoch milliseconds, UTC**, in columns named `*_at`. This gives exact ordering, cheap range queries ("two charges within N minutes"), and values that are safe as JS numbers in D1 results. Take them from Restate's journaled clock (`ctx.now()`), never from `SystemTime`. |
| Simulated time | Each order has a virtual clock (`order_clock.offset_secs`). Instants on the order's simulated timeline are `sim_*_at`, also in epoch ms. Simulated calendar dates (`shipped_on`, `eta`, `issue_date`) are `TEXT` `YYYY-MM-DD`. Simulated now is real now plus the offset (`domain::VirtualClock`). |
| Dates | `TEXT` `YYYY-MM-DD`, checked with `col IS date(col)`. |
| Money | `INTEGER` cents EUR, VAT-inclusive, in columns named `*_cents`. Payment amounts are positive, and `kind` (`charge` or `refund`) gives the direction. Credit notes store negative amounts on the document and its lines. |
| Booleans | `INTEGER` `0`/`1` with a CHECK. |
| JSON | `TEXT` named `*_json`, with `CHECK (json_valid(…))`. The Rust type is named in a trailing comment. Read and write it with serde (`techdepo_ports::sql::column::json_text`, `Param::json`). |
| Enums | `TEXT` with `CHECK (col IN ('wire', …))`, always in exactly that form. The lists mirror the Rust enums' wire names: `techdepo-domain` for order status, intent, ticket status, actor, invoice kind and status, and `techdepo_api_types::vocab` for everything else. The test `vocab::tests::migration_checks_match_enums` fails when they drift. A `NULL` passes a CHECK, so optional enum columns use the same form without `IS NULL OR`. |
| Strictness | All tables are `STRICT`. D1 always enforces foreign keys. |

## Idempotent writes

Writes run inside `ctx.run`, which can execute again when a run fails after the database has
committed but before Restate journaled the result. Every write must therefore be safe to repeat:

- **Inserts use deterministic keys** plus `ON CONFLICT DO NOTHING`, or an upsert. IDs come from
  `ctx.rand_uuid()`, which is journaled, or from the input. Append-only tables take a `seq` that
  their single writer assigns from its Restate state: `order_events` is written only by the
  `Order` VO, and `ticket_messages` only by `SupportTicket`. `carrier_events` takes its `seq` from
  the carrier code that owns the shipment.
- **Multi-row changes go in one `SqlExecutor::batch`**, which D1 runs as a single transaction.
- **Sequences** (`order_sequences`, `invoice_sequences`). First check whether the entity already
  exists by its deterministic ID, and return its number if it does. Otherwise allocate and insert
  in one batch:

  ```sql
  INSERT INTO invoice_sequences (org_id, year, last_seq) VALUES (?1, ?2, 1)
    ON CONFLICT (org_id, year) DO UPDATE SET last_seq = last_seq + 1
    RETURNING last_seq;
  -- then, in the same batch:
  INSERT INTO invoices (…, number, year, seq, …)
    VALUES (…, printf('TD-%04d-%06d', ?2, (SELECT last_seq FROM invoice_sequences WHERE org_id = ?1 AND year = ?2)), ?2,
            (SELECT last_seq FROM invoice_sequences WHERE org_id = ?1 AND year = ?2), …);
  ```

  Order numbers work the same way: the first order is `TD-1001`
  (`INSERT … VALUES (?1, 1001) ON CONFLICT (org_id) DO UPDATE SET last_number = last_number + 1`).
  Invoices and credit notes share one sequence per year.
- **Stock** is decremented without a guard (`UPDATE inventory SET stock = stock - ?`). The
  `stock >= 0` CHECK aborts the whole batch on oversell, which the repository reports as out of
  stock.
- **Counters** (`llm_usage`) are upsert-increments. A retried run may double-count a call, which
  is acceptable for budgets.

## Tables

| Table | Key | Written by | Notes |
|---|---|---|---|
| `orgs` | `id` | `Org` VO (`provision`, `update_settings`, `expire`; `reset_demo` and `reseed_stock` stamp `catalog_version`) | `settings_json` is `domain::OrgSettings`. Missing fields default. `expires_at` (`0003`) is set at provisioning when `ORG_EXPIRY_DAYS` is on. `features_json` (`0004`) is `[domain::OrgFeature]`, set by hand (no handler writes it); unknown names are ignored. |
| `org_members` | `(org_id, user_id)` | web worker (identity cache) | Unique `(org_id, email)` with the email lower-cased. Sender verification in `AUTH_MODE=dev`. |
| `inventory` | `(org_id, product_id)` | `Org` VO (`provision`, `set_stock`, `reset_demo`, `reseed_stock`), `Order` VO (`place`) | Seeded from `Product::default_stock`. `orgs.catalog_version` records the catalog version it was seeded from, and new products are added with `INSERT OR IGNORE`. |
| `carts`, `cart_items` | `(org_id, user_id[, product_id])` | `Customer` VO | Quantity 1–99 per product. |
| `builds` | `(org_id, id)` | `Customer` VO | `product_ids_json` is a JSON array of product IDs, in slot order. |
| `customer_addresses` | `(org_id, id)` | `Customer` VO | `address_json` is `api_types::Address`. |
| `order_sequences` | `org_id` | `Order` VO (`place`) | Not reset by demo reset. |
| `orders` | `(org_id, id)`, unique `(org_id, number)` | `Order` VO | Flags are columns. `billing_address_json` is `NULL` when it is the same as shipping. A cancelled order is kept and hidden from default lists. |
| `order_items` | `(org_id, order_id, line_no)` | `Order` VO | Snapshot of product name, SKU, category and price. |
| `order_events` | `(org_id, order_id, seq)` | `Order` VO only (others call `Order/record_event`) | `at` is real time and `sim_at` is the order's simulated time. `ref_kind`/`ref_id` link to the related record. |
| `payments` | `(org_id, id)`, unique `provider_ref` | `Payments` service | Ledger of charges and refunds. A refund references its charge (`refund_of`). Duplicate charges are detected at read time. |
| `invoice_sequences` | `(org_id, year)` | `Invoicing` | Shared by invoices and credit notes. |
| `invoices`, `invoice_lines` | `(org_id, id)`, unique `(org_id, number)` | `Invoicing` | Immutable. `html` is the document as issued. A fix is a credit note (`credit_for_id`) plus a reissued invoice (`supersedes_id`), and the original becomes `voided`. At most one invoice supersedes a given one (`0002`). |
| `shipments`, `carrier_events` | `(org_id, id)` / `(org_id, shipment_id, seq)` | `Carrier` service | `lost_outcome` and `trace_status` are hidden truth. Events with `internal = 1` are hidden from the customer. One shipment per order. |
| `tickets`, `ticket_messages` | `(org_id, id)` / `(org_id, ticket_id, seq)` | `SupportTicket` VO, `Router` (classification columns) | `classification_json` is `api_types::Classification`. `awaiting` is set exactly when the status is `awaiting_customer`. AI drafts awaiting review are `draft` messages. |
| `investigations` | `(org_id, id)` | `Investigation` workflow | `ticket_id` is `NULL` when an admin opened it from an order. |
| `refunds` | `(org_id, id)` | `RefundWorkflow` | `evidence_json` is `[api_types::Evidence]`. |
| `human_tasks` | `(org_id, id)` | `HumanTasks` service | `allowed_decisions_json` is `[DecisionKind]`, `context_json` is `TaskContext`, and `decision_json` is `HumanDecision`. `awakeable_id` is never sent to a browser, and is set exactly for the kinds a workflow waits on (`approve_refund`, `investigation_decision`); the other kinds belong to a ticket. |
| `email_log` | `id` | mail-in (inbound), `Email` service (outbound) | Rejected inbound mail is kept with its `reject_reason`. Unique `(org_id, direction, message_id)` dedupes redelivery. |
| `agent_runs`, `agent_steps` | `(org_id, id)` / `(org_id, run_id, idx)` | agents, `Router`, judge | The trace. Inputs and outputs are JSON. |
| `llm_usage` | `(org_id, day, model)` | the LLM budget check | UTC day. The global budget sums over `day`. |
| `app_settings` | `key` | `Muhaha` service (`update_models`) | Global, no `org_id`. `models` is `domain::ModelSettings`; a missing row means the deployment's defaults. Read by the services once per invocation, journaled, and by no one else. |
| `demo_faults` | `(org_id, order_id)` | `Order` VO (`set_faults`, `place`) | `faults_json` is `domain::FaultToggles`, seeded from the org settings at placement. |
| `order_clock` | `(org_id, order_id)` | `Order` VO (`advance_clock`, `place`) | Offset in seconds, which never decreases. |

## Demo reset

`Org/reset_demo` deletes one org's operational data in a single batch, in this order, because
some foreign keys have no cascade on purpose (the operator's `Org/clear_data` runs the deletes
alone, `Org/reseed_stock` the restock alone; `Muhaha/reset_all` resets every org):

```sql
DELETE FROM human_tasks    WHERE org_id = ?1;
DELETE FROM agent_runs     WHERE org_id = ?1;   -- cascades to agent_steps
DELETE FROM refunds        WHERE org_id = ?1;
DELETE FROM investigations WHERE org_id = ?1;
DELETE FROM tickets        WHERE org_id = ?1;   -- cascades to ticket_messages
DELETE FROM orders         WHERE org_id = ?1;   -- cascades to items, events, payments, invoices (+ lines),
                                                -- shipments (+ carrier events), demo_faults, order_clock
DELETE FROM email_log      WHERE org_id = ?1;
UPDATE inventory SET stock = <default>, updated_at = ?2 WHERE org_id = ?1 AND product_id = ?3;  -- per product
```

The reset keeps `orgs` and their settings, `org_members`, carts, builds, addresses,
`llm_usage` (budgets survive resets), and both sequences, so old emails never point at a new
order or invoice.

Deleting the `orgs` row cascades to everything. `Org/expire` (`ORG_EXPIRY_DAYS`) does it in one
batch after the reset's deletes above, in their order (`Orgs::delete_statements`); a Clerk
`organization.deleted` hook could do the same later. Restate state of the org's objects (orders,
tickets) is left behind: nothing reads it once the rows are gone, and a re-provisioned org gets a
new public ID, so no ticket key repeats.
