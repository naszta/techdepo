# API contract

The `/api/*` routes of `techdepo-web`, the Restate handlers they call, and the conventions both
sides follow. The source of truth is the code; this page is the readable version:

| What | Where |
|---|---|
| Route table (method, path, access, Restate handler) and typed endpoints | `techdepo_api_types::endpoints` (`ALL`, one type per route implementing `Endpoint`) |
| Request and response DTOs | `techdepo_api_types::{session, catalog, account, orders, demo, support, tasks, cases, admin}` |
| Error body | `techdepo_api_types::ApiError` / `ErrorCode` |
| Restate ingress handlers, keys, inputs and outputs | `techdepo_api_types::ingress` |
| Enums stored in D1 | `techdepo_api_types::vocab` (mirrored by the migrations' CHECKs) |
| D1 schema | `migrations/0001_init.sql`, conventions in `migrations/README.md` |
| Ports | `techdepo_ports`: `SqlExecutor`, `Llm`, `Classifier`, `Mailer` (services); `MemberDirectory` (mail-in's sender check only) |

Tests keep this page honest: `endpoints::tests::api_doc_lists_every_route` fails if a route or
handler below is missing, and `vocab::tests::migration_checks_match_enums` fails if an enum and
its CHECK constraint drift.

## Conventions

### Authentication and access

Every route except `Public` and `DevOnly` needs `Authorization: Bearer <token>`.

| Access | Who | Enforcement |
|---|---|---|
| `Public` | anyone | none |
| `DevOnly` | anyone, only when `AUTH_MODE=dev` | `404` in any other mode |
| `Member` | a signed-in member of the active org, any role | `401` without a valid token. Data is scoped to the org, and customer routes (cart, builds, addresses, orders, tickets, demo controls) are scoped to the caller's own records. A record that exists but belongs to someone else is `404`, never `403`. The demo controls are the exception: an `org:admin` may drive any order of the org. |
| `Admin` | an `org:admin` of the active org | `403` for other members. All `/api/admin/*` routes but the Muhaha ones, and only those. |
| `Operator` | an `org:admin` of an org with `OrgFeature::Muhaha` (switched on by hand, `orgs.features_json`) | `404` for everyone else, members and admins of other orgs alike, so the routes stay unknown. All `/api/admin/muhaha/*` routes, and only those. They act across every org and on app-wide settings. |

- **`AUTH_MODE=clerk`:** `ClerkAuthLayer` verifies the session JWT against JWKS. The org ID
  and role come from the token's org claims. A token without an active org is `403`
  (Clerk's "membership required" setting normally prevents that).
- **`AUTH_MODE=dev`:** `POST /api/dev/login` returns an unsigned token encoding
  `org_dev_<hash(org name)>` and `user_dev_<hash(email)>`. Never enabled in production.
- **Member cache:** every authenticated request upserts the caller into `org_members`
  (`last_seen_at`, email, name, role). This is the one D1 write the web worker does itself:
  it is an identity cache, not business state. In `AUTH_MODE=dev` it is also mail-in's member
  directory.
- **Lazy provisioning:** `GET /api/me` (and the dev login) calls `Org/provision` when the
  org has no `orgs` row. The SPA calls `/api/me` first after sign-in. With `ORG_EXPIRY_DAYS`
  set, provisioning also schedules the org's deletion (`OrgInfo.expires_at`, internal
  `Org/expire`); a member of an expired org gets a fresh shop on the next request.

### Requests and responses

- JSON everywhere, `snake_case` fields. `GET`/`DELETE` send no body. `POST`/`PUT` send a JSON
  body, which may be `{}`.
- Instants are RFC 3339 strings in UTC. Calendar dates (simulated ship date, ETA, invoice
  date) are `YYYY-MM-DD`. Money is integer cents EUR, VAT-inclusive. Order numbers are
  `"TD-1042"`, ticket IDs `"T-7F2M9Q"`, invoice numbers `"TD-2026-000042"`.
- Path parameters: `{number}` is an order number (`TD-1042`), `{invoice}` an invoice number,
  `{id}` the record's ID (tickets: `T-7F2M9Q`), `{product_id}` a catalog slug, `{user_id}` a
  Clerk user ID.
- **Lists** take `cursor` and `limit` (default 25, max 100) and return
  `Page<T> { items, next_cursor? }`. The cursor is opaque (the server encodes the sort key of
  the last row).
- **Validation:** every body type implements `Validate`. The SPA runs it before submitting,
  and the web worker runs it again before calling Restate.

### Errors

Every non-2xx response is an `ApiError { code, message, fields? }`. `message` is safe to show.

| `code` | Status | When |
|---|---|---|
| `bad_request` | 400 | Malformed JSON, query or path |
| `unauthenticated` | 401 | No or invalid token |
| `forbidden` | 403 | Not `org:admin` on an admin route |
| `not_found` | 404 | Unknown route, or a record not in this org / not the caller's |
| `conflict` | 409 | Generic state conflict |
| `invalid_transition` | 409 | The order state machine does not allow the action now |
| `out_of_stock` | 409 | Checkout: not enough stock |
| `cart_changed` | 409 | Checkout: `expected_total` does not match the cart |
| `incompatible_build` | 409 | Checkout: compatibility errors and no `override_compat` |
| `already_decided` | 409 | The human task was already decided |
| `validation` | 422 | `fields` lists every invalid field (`shipping_address.city`, …) |
| `rate_limited` | 429 | Rate limit or LLM budget |
| `unavailable` | 503 | Restate or D1 unreachable, or the handler's retries were exhausted; retry later |
| `internal` | 500 | A bug |

Restate handlers reject requests with a **terminal error** whose code is the HTTP status and
whose message is the `ApiError` JSON (`ingress::terminal_message`). The web worker passes it
through unchanged (`ingress::api_error_from_terminal`).

### Mutations and idempotency

- **Every mutation goes through a Restate handler.** The web worker never writes business
  state. It validates, calls the handler request-response through ingress, then reads fresh
  data back from D1 for the response.
- **Workflows are started one-way** (`/send`): starting a refund or an investigation returns
  its ID immediately (`RefundStarted`, `InvestigationStarted`). The workflow writes its D1
  record in its first step. The web form's `SupportTicket/inbound` is also one-way, as with
  mail-in.
- **Client keys:** creates that must not happen twice carry an `idempotency_key` in the body
  (a UUID per form load): builds, addresses, checkout, support messages, scenarios, refunds and
  investigations. The web worker sends
  `Idempotency-Key: <org>:<user>:<route name>:<client key>` (`ingress::idempotency_key`). IDs
  that must be known before the handler runs are derived from the same parts
  (`ingress::derive_id`: `bld_…`, `adr_…`, `ref_…`, `inq_…`).
- Other mutations (cart, demo controls, settings, stock, task decisions, ticket actions) are
  naturally idempotent or meant to repeat, and send no `Idempotency-Key`. `HumanTasks/decide`
  rejects a second decision with `already_decided`.

### Rate limits

Public demo cost and abuse controls (`apps/web-worker/src/throttle.rs`), all `429 rate_limited`:

- **Burst limit per user:** every mutating route (a non-`GET` route that calls Restate) counts
  against `<org>:<user>` before the caller is resolved: 60 per minute, through the Workers Rate
  Limiting binding `MUTATION_LIMITER` (`[[ratelimits]]` in `apps/web-worker/wrangler.toml`,
  counted per Cloudflare location). Reads are not limited. A worker without the binding limits
  nothing, and a failing binding lets the request through.
- **Orders per org and hour:** checkout and scenario presets are refused once the org created
  `MAX_ORDERS_PER_HOUR` orders (default 50, any kind, counted in D1) in the last hour. Each
  order starts a workflow that runs for days and sends emails. A retry of a checkout that went
  through just as the cap was reached is refused too; the order is in the list.
- **Support messages per org and hour:** `MAX_INBOUND_PER_HOUR`, shared with mail-in (see
  `POST /api/support/messages`).

## Routes

`Empty` means no query parameters, or an empty body (`{}` for `POST`/`PUT`, none for
`GET`/`DELETE`). "Calls" is the Restate handler (`Service/handler`, see
[Restate ingress handlers](#restate-ingress-handlers)); `—` means a read straight from D1 or a
pure computation.

### Session

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/config` | Public | `GetConfig` | → `Config` | — | `auth_mode`, Clerk publishable key, support domain, catalog version, LLM backend, Restate UI URL (local). Lets one SPA build run everywhere. |
| `POST /api/dev/login` | DevOnly | `DevLogin` | `DevLoginRequest` → `DevLoginResponse` | `Org/provision` | Returns the dev token and `Me`. Provisions the dev org on first use. |
| `GET /api/me` | Member | `GetMe` | → `Me` | `Org/provision` | User, org (with `support_address`, `expires_at` when it expires, and its `features`) and role. Calls `Org/provision` only when the org is unknown. |

### Catalog and compatibility

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/catalog` | Public | `GetCatalog` | → `CatalogResponse` | — | Every product (typed specs, no long-form content). `ETag: "<CATALOG_VERSION>"`, long cache. The SPA also has the catalog compiled in. |
| `GET /api/stock` | Member | `GetStock` | → `StockResponse` | — | Per-org stock with `in_stock`/`low`/`out_of_stock` levels. |
| `GET /api/products/{id}` | Member | `GetProduct` | → `ProductDetail` | — | Product, rich content (overview, who it's for, pros, cons, FAQ), stock, "works well with". |
| `POST /api/compat/check` | Member | `CheckCompat` | `CompatCheckRequest` → `CompatCheckResponse` | — | `compat::check_build` (+ `suggest_fixes` if asked). Pure. The SPA usually runs the engine locally. |
| `POST /api/compat/alternatives` | Member | `FindAlternatives` | `AlternativesRequest` → `AlternativesResponse` | — | `compat::alternatives` within a price band. Pure. |

### Cart, builds, addresses, checkout

All scoped to the caller. `Customer` is keyed `<org_id>:<user_id>`.

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/cart` | Member | `GetCart` | → `Cart` | — | Lines priced from the catalog, stock, totals, compat report of build-relevant parts. |
| `POST /api/cart/items` | Member | `AddToCart` | `AddToCartRequest` → `Cart` | `Customer/add_to_cart` | Adds to an existing line. Quantity per line 1–99. |
| `PUT /api/cart/items/{product_id}` | Member | `SetCartItem` | `SetCartItemRequest` → `Cart` | `Customer/set_cart_item` | `0` removes the line. |
| `DELETE /api/cart/items/{product_id}` | Member | `RemoveCartItem` | → `Cart` | `Customer/set_cart_item` | Quantity 0. |
| `DELETE /api/cart` | Member | `ClearCart` | → `Cart` | `Customer/clear_cart` | |
| `GET /api/builds` | Member | `ListBuilds` | → `BuildList` | — | Most recently saved first, with verdict and total. |
| `POST /api/builds` | Member | `CreateBuild` | `CreateBuildRequest` → `BuildDetail` | `Customer/save_build` | Client key; ID `derive_id("bld", …)`. |
| `GET /api/builds/{id}` | Member | `GetBuild` | → `BuildDetail` | — | Live report; parts no longer in the catalog are listed in `removed`. |
| `PUT /api/builds/{id}` | Member | `UpdateBuild` | `UpdateBuildRequest` → `BuildDetail` | `Customer/save_build` | Absent fields unchanged. |
| `DELETE /api/builds/{id}` | Member | `DeleteBuild` | → `Deleted` | `Customer/delete_build` | |
| `POST /api/builds/{id}/add-to-cart` | Member | `AddBuildToCart` | → `Cart` | `Customer/add_build_to_cart` | |
| `GET /api/addresses` | Member | `ListAddresses` | → `AddressList` | — | |
| `POST /api/addresses` | Member | `CreateAddress` | `SaveAddressRequest` → `SavedAddress` | `Customer/save_address` | Client key; ID `derive_id("adr", …)`. |
| `PUT /api/addresses/{id}` | Member | `UpdateAddress` | `SaveAddressRequest` → `SavedAddress` | `Customer/save_address` | Replaces it. |
| `DELETE /api/addresses/{id}` | Member | `DeleteAddress` | → `Deleted` | `Customer/delete_address` | |
| `POST /api/checkout` | Member | `PlaceOrder` | `PlaceOrderRequest` → `PlaceOrderResponse` | `Customer/checkout` | Client key. `Customer/checkout` re-checks stock, `expected_total` and compatibility (409s above), calls `Order/place` (reserves stock, writes the order in `awaiting_payment`, seeds `demo_faults` from the org's default faults), then empties the cart. Counts against the org's `MAX_ORDERS_PER_HOUR` (`429 rate_limited`, [Rate limits](#rate-limits)). |

### Orders and demo controls

Scoped to the caller's orders (`404` otherwise). `Order` is keyed by order ID; the web worker
resolves `{number}` to the ID in D1 first.

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/orders` | Member | `ListOrders` | `OrderListQuery` → `Page<OrderSummary>` | — | Newest first. Cancelled orders hidden unless `include_cancelled=true`. |
| `GET /api/orders/{number}` | Member | `GetOrder` | → `OrderDetail` | — | Items, totals with VAT, addresses, customer-visible timeline, card statement, invoices, tracking (a lost parcel shows as `in_transit`, no internal carrier events), linked tickets, simulated now, `mailto:` support link. |
| `GET /api/orders/{number}/invoices/{invoice}` | Member | `GetOrderInvoice` | → `InvoiceDocument` | — | The document as issued (HTML). |
| `GET /api/orders/{number}/demo` | Member | `GetDemoPanel` | → `DemoPanel` | — | Valid next demo actions only, armed faults, simulated clock, hidden truth. |
| `POST /api/orders/{number}/demo/actions` | Member | `RunDemoAction` | `RunDemoActionRequest` → `DemoPanel` | `Order/demo_action` | `invalid_transition` if the action is not valid now. Faults fire here (double charge, declined, lost, …). |
| `PUT /api/orders/{number}/demo/faults` | Member | `SetDemoFaults` | `SetFaultsRequest` → `DemoPanel` | `Order/set_faults` | Replaces the toggles. Email outage 1–60 minutes. |
| `POST /api/orders/{number}/demo/clock` | Member | `AdvanceDemoClock` | `AdvanceClockRequest` → `DemoPanel` | `Order/advance_clock` | `day`, `workday`, `next_monday_9am`, or `ship_last_friday` (shipped orders). Delivers when the clock passes the ETA and the parcel is not lost. |
| `POST /api/demo/scenarios` | Member | `CreateScenario` | `CreateScenarioRequest` → `CreateScenarioResponse` | `Demo/create_scenario` | M7 presets: an order for the caller already in the chosen state. Counts against the org's `MAX_ORDERS_PER_HOUR` with checkout (`429 rate_limited`). |

### Support (customer)

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/tickets` | Member | `ListTickets` | `TicketListQuery` → `Page<TicketSummary>` | — | The caller's tickets, most recently updated first. |
| `GET /api/tickets/{id}` | Member | `GetTicket` | → `TicketDetail` | — | Only `received`/`sent` messages; no drafts, classification or traces. |
| `POST /api/support/messages` | Member | `SendSupportMessage` | `SendSupportMessageRequest` → `SendSupportMessageResponse` | `SupportTicket/inbound` | The "Send as email" form: same pipeline as mail-in with `channel = web`, authenticated by the session instead of DMARC and membership. `Message-ID` is `web.<client key>@<domain>`; a new ticket's ID is `TicketId::derive(public org, Message-ID)`. Counts against the org's `MAX_INBOUND_PER_HOUR` with mail-in (`429 rate_limited`; a retry of an accepted message is not counted). One-way call; poll the ticket. |

### Admin

`org:admin` only.

| Route | Access | Endpoint | Query / body → response | Calls | Notes |
|---|---|---|---|---|---|
| `GET /api/admin/overview` | Admin | `GetAdminOverview` | → `AdminOverview` | — | Orders by status, open tickets, pending and overdue tasks, simulated revenue, refunds, the oldest pending tasks, recent activity. |
| `GET /api/admin/tasks` | Admin | `ListTasks` | `TaskListQuery` → `Page<HumanTaskSummary>` | — | Default `status=pending`, oldest first. |
| `GET /api/admin/tasks/{id}` | Admin | `GetTask` | → `HumanTaskDetail` | — | Context (per kind), evidence, allowed decisions, the recorded decision. Never the awakeable ID. |
| `POST /api/admin/tasks/{id}/decide` | Admin | `DecideTask` | `DecideTaskRequest` → `HumanTaskDetail` | `HumanTasks/decide` | Validated against the task's allowed decisions (`validation`), `already_decided` if decided. Resolves the workflow's awakeable (refund approval, investigation decision) or forwards the decision to `SupportTicket/task_decided` (reply review, escalation, offers declined). |
| `GET /api/admin/orders` | Admin | `ListAdminOrders` | `AdminOrderListQuery` → `Page<AdminOrderSummary>` | — | Filter by status, customer, flags; `q` matches order number or email. |
| `GET /api/admin/orders/{number}` | Admin | `GetAdminOrder` | → `AdminOrderDetail` | — | The full order page including internal events, hidden shipment truth, faults, investigations, refunds, tasks, the demo panel and the admin actions that apply. |
| `POST /api/admin/orders/{number}/cancel` | Admin | `CancelOrder` | `AdminCancelOrderRequest` → `OrderUpdated` | `Order/cancel` | Unpaid orders only; paid ones are refunded instead (`invalid_transition`). |
| `POST /api/admin/orders/{number}/refunds` | Admin | `InitiateRefund` | `InitiateRefundRequest` → `RefundStarted` | `RefundWorkflow/run` | One-way start. Pre-approved (a person initiated it); `RefundWorkflow` still executes it. Key `derive_id("ref", …)`. The route runs the workflow's own `plan_refund` first (open refunds count against their charge), so a refund the workflow would reject is `422`/`409` here; `409` while another refund of the order is pending. |
| `POST /api/admin/orders/{number}/investigations` | Admin | `OpenInvestigation` | `OpenInvestigationRequest` → `InvestigationStarted` | `Investigation/run` | One-way start, no ticket. Shipped, undelivered orders without an open investigation (`invalid_transition` otherwise). Key `derive_id("inq", …)`. |
| `GET /api/admin/tickets` | Admin | `ListAdminTickets` | `AdminTicketListQuery` → `Page<AdminTicketSummary>` | — | Filter by status, intent, assignee, customer. |
| `GET /api/admin/tickets/{id}` | Admin | `GetAdminTicket` | → `AdminTicketDetail` | — | Thread including drafts, classification with per-intent probabilities, route, reply confidence, tasks, investigations, refunds, trace summaries. |
| `GET /api/admin/tickets/{id}/trace` | Admin | `GetTicketTrace` | → `TicketTrace` | — | Every run with every step (LLM calls, tool calls, classifier passes, guardrails) and their JSON inputs and outputs. |
| `POST /api/admin/tickets/{id}/actions` | Admin | `ActOnTicket` | `TicketActionRequest` → `TicketUpdated` | `SupportTicket/human` | `reply` (takes over), `take_over`, `hand_back` (optional `intent`: the unanswered messages are routed under it at once), `close`. Cancels the ticket's pending escalation, reply-review and offers-declined tasks: acting on the ticket answers them. |
| `GET /api/admin/investigations` | Admin | `ListInvestigations` | `InvestigationListQuery` → `Page<InvestigationSummary>` | — | |
| `GET /api/admin/investigations/{id}` | Admin | `GetInvestigation` | → `InvestigationDetail` | — | Shipment with internal events, trace state, carrier responses, tasks, refund or replacement. |
| `GET /api/admin/refunds` | Admin | `ListRefunds` | `RefundListQuery` → `Page<RefundSummary>` | — | |
| `GET /api/admin/refunds/{id}` | Admin | `GetRefund` | → `RefundDetail` | — | Evidence, the reversed charge, the refund entry, credit note, approver. |
| `GET /api/admin/invoices` | Admin | `ListInvoices` | `InvoiceListQuery` → `Page<InvoiceSummary>` | — | Invoices and credit notes; filter by status (issued/voided) and kind. |
| `GET /api/admin/invoices/{id}` | Admin | `GetInvoice` | → `InvoiceDocument` | — | |
| `POST /api/admin/invoices/{id}/void-reissue` | Admin | `VoidReissueInvoice` | `VoidReissueRequest` → `InvoiceReissued` | `Invoicing/void_and_reissue` | Credit note plus corrected invoice. No money moves, so no approval. `invalid_transition` for a voided invoice or a credit note, also for the loser of two concurrent calls (a voided invoice has one replacement, enforced by a unique index). |
| `GET /api/admin/payments` | Admin | `GetPayments` | `PaymentListQuery` → `PaymentsLedger` | — | Ledger rows with `duplicate_of`, plus every suspected duplicate pair (`admin::find_duplicate_charges`, deterministic: same order, same amount, within 15 minutes). |
| `GET /api/admin/inventory` | Admin | `GetInventory` | `InventoryQuery` → `InventoryList` | — | Stock, default stock, reserved units, low-stock level. Prices are global and read-only. |
| `PUT /api/admin/inventory/{product_id}` | Admin | `SetStock` | `SetStockRequest` → `InventoryItem` | `Org/set_stock` | 0–9999. |
| `GET /api/admin/customers` | Admin | `ListCustomers` | `CustomerListQuery` → `Page<CustomerSummary>` | — | From the `org_members` cache, with order and ticket counts. |
| `GET /api/admin/customers/{user_id}` | Admin | `GetCustomer` | → `CustomerDetail` | — | |
| `GET /api/admin/emails` | Admin | `ListEmails` | `EmailListQuery` → `Page<EmailSummary>` | — | Inbound (including rejected, with the reason) and outbound. |
| `GET /api/admin/emails/{id}` | Admin | `GetEmail` | → `EmailDetail` | — | Headers and bodies as sent or received. |
| `GET /api/admin/settings` | Admin | `GetSettings` | → `SettingsResponse` | — | Current settings and defaults. |
| `PUT /api/admin/settings` | Admin | `UpdateSettings` | `UpdateSettingsRequest` → `SettingsResponse` | `Org/update_settings` | Full replacement, validated by `OrgSettings::validate`. |
| `POST /api/admin/demo/reset` | Admin | `ResetDemo` | `DemoResetRequest` → `DemoResetResponse` | `Org/reset_demo` | `confirm` must be `RESET`. Deletes orders, tickets, invoices, tasks, traces and the email log, and restores default stock (see `migrations/README.md`). |

### Operator (Muhaha)

The Muhaha page (`/admin/muhaha`), for orgs with `OrgFeature::Muhaha` only. The org routes take
any org's ID: they are the operator's, across tenants.

| Route | Access | Endpoint | Body → response | Restate | Notes |
|---|---|---|---|---|---|
| `GET /api/admin/muhaha/orgs` | Operator | `ListMuhahaOrgs` | → `MuhahaOrgList` | — | Every org, newest first, with member, order and ticket counts and whether its stock was seeded from the current catalog. |
| `POST /api/admin/muhaha/reset` | Operator | `ResetAllOrgs` | `DemoResetRequest` → `ResetAllResponse` | `Muhaha/reset_all` | `confirm` must be `RESET`. `Org/reset_demo` for every org, one after another, oldest first. |
| `POST /api/admin/muhaha/orgs/{org_id}/clear` | Operator | `ClearOrg` | `DemoResetRequest` → `DemoResetResponse` | `Muhaha/clear_org` | `confirm` must be `RESET`. Deletes what the demo reset deletes; the stock stays. `404` for an unknown org. |
| `POST /api/admin/muhaha/orgs/{org_id}/reseed` | Operator | `ReseedOrg` | → `ReseedResponse` | `Muhaha/reseed_org` | Every catalog product back at its default stock (products added since the org was seeded included), and the org marked as seeded from the current catalog. |
| `GET /api/admin/muhaha/models` | Operator | `GetModels` | → `ModelsResponse` | `Muhaha/models` | The model settings in effect, the deployment's defaults, and which providers have credentials (only the services worker knows its secrets, so this read goes through Restate). |
| `PUT /api/admin/muhaha/models` | Operator | `UpdateModels` | `UpdateModelsRequest` → `ModelsResponse` | `Muhaha/update_models` | Full replacement, validated by `ModelSettings::validate`; `settings: null` goes back to the defaults. A provider without credentials is refused (`validation`). Applies to invocations that start afterwards: each reads the settings once, journaled. |

## Restate ingress handlers

Everything the web worker, mail-in and xtask may call (`ingress::ALL`). Every other service is
`ingress_private`. Paths are `/{Service}/{handler}` or `/{Object}/{key}/{handler}`, with
`/send` appended for one-way calls (`Handler::call_path` / `send_path`).

| Handler | Kind | Key | Input → output | Called by |
|---|---|---|---|---|
| `Org/provision` | object | org ID | `ProvisionOrgInput` → `OrgInfo` | `GET /api/me`, dev login |
| `Org/update_settings` | object | org ID | `UpdateSettingsInput` → `OrgSettings` | settings page |
| `Org/set_stock` | object | org ID | `SetStockInput` → `StockEntry` | inventory page |
| `Org/reset_demo` | object | org ID | `ResetDemoInput` → `DemoResetResponse` | settings page, `Muhaha/reset_all` |
| `Customer/add_to_cart` | object | `<org>:<user>` | `CartItemInput` → `()` | cart |
| `Customer/set_cart_item` | object | `<org>:<user>` | `CartItemInput` → `()` | cart |
| `Customer/clear_cart` | object | `<org>:<user>` | `CustomerInput` → `()` | cart |
| `Customer/save_build` | object | `<org>:<user>` | `SaveBuildInput` → `()` | builder |
| `Customer/delete_build` | object | `<org>:<user>` | `BuildRefInput` → `()` | builds |
| `Customer/add_build_to_cart` | object | `<org>:<user>` | `BuildRefInput` → `()` | builds |
| `Customer/save_address` | object | `<org>:<user>` | `SaveAddressInput` → `()` | profile |
| `Customer/delete_address` | object | `<org>:<user>` | `AddressRefInput` → `()` | profile |
| `Customer/checkout` | object | `<org>:<user>` | `CheckoutInput` → `PlacedOrder` | checkout |
| `Order/demo_action` | object | order ID | `DemoActionInput` → `OrderStatus` | demo controls |
| `Order/set_faults` | object | order ID | `SetFaultsInput` → `()` | demo controls |
| `Order/advance_clock` | object | order ID | `AdvanceClockInput` → `ClockAdvanced` | demo controls |
| `Order/cancel` | object | order ID | `CancelOrderInput` → `OrderStatus` | admin (also the `CancelOrder` route, internally) |
| `SupportTicket/inbound` | object | `<org>:<ticket>` | `InboundMessage` → `()` | mail-in (`/send`, `Idempotency-Key: <Message-ID>`), web form |
| `SupportTicket/human` | object | `<org>:<ticket>` | `HumanTicketInput` → `TicketState` | admin ticket page |
| `HumanTasks/decide` | service | — | `DecideTaskInput` → `TaskDecided` | human tasks inbox |
| `Invoicing/void_and_reissue` | service | — | `VoidReissueInput` → `InvoiceReissued` | admin invoices (also `BillingIssue`, internally) |
| `RefundWorkflow/run` | workflow | refund ID | `StartRefundInput` → `RefundFinal` | admin (`/send`); support routes internally |
| `Investigation/run` | workflow | investigation ID | `StartInvestigationInput` → `InvestigationFinal` | admin (`/send`); `WhereIsMyOrder` internally |
| `Demo/create_scenario` | service | — | `CreateScenarioInput` → `PlacedOrder` | demo scenarios |
| `Muhaha/reset_all` | service | — | `OperatorInput` → `ResetAllResponse` | Muhaha page |
| `Muhaha/clear_org` | service | — | `OrgTargetInput` → `DemoResetResponse` | Muhaha page (calls the internal `Org/clear_data`) |
| `Muhaha/reseed_org` | service | — | `OrgTargetInput` → `ReseedResponse` | Muhaha page (calls the internal `Org/reseed_stock`) |
| `Muhaha/models` | service | — | `OperatorInput` → `ModelsResponse` | Muhaha page |
| `Muhaha/update_models` | service | — | `UpdateModelsInput` → `ModelsResponse` | Muhaha page |

Rules for handler authors:

- Inputs carry `org_id` (and the acting `user_id`) from the verified session or email. A keyed
  handler checks that its key belongs to that org (`ingress::keys::split`) and that the record
  it touches has that `org_id`; a mismatch is a terminal `404`.
- Invalid state transitions are terminal `409 invalid_transition` errors, never retried.
- Handlers whose input or output is `()` take or return no body.
- Internal handlers (`Order/place`, `Order/record_event`, `Order/mark_refunded`,
  `SupportTicket/{route_result, classified, timeout, task_decided, get}`, `Router/route`, the routes'
  `<Route>/run` (`support::contract`), agents, `Notifications`, `Email`,
  `Payments`, `Carrier`) are not part of this contract; their types live in
  `crates/services`.

## Ports

`crates/services` depends only on these traits (`techdepo_ports`), so it builds for wasm32
and natively:

| Port | Method(s) | Production adapter | Local / test |
|---|---|---|---|
| `SqlExecutor` | `execute`, `query_all`, `query_first`, `batch` (one transaction); typed reads through `SqlExecutorExt::query_as` | D1 (`crates/db`) | rusqlite (post-plan) |
| `Llm` | `chat(ChatRequest) -> ChatResponse` (text, tool calls, usage) | Workers AI via allama (`crates/llm`) | `crates/llm` mock, `testing::ScriptedLlm` |
| `Classifier` | `classify(text, questions) -> Classification` (choice with per-option probabilities, noul, score) | Workers AI Clef | `crates/llm` mock |
| `MemberDirectory` (mail-in only) | `is_member(org, email)`, `list_members(org)` | Clerk Backend API + KV | D1 `org_members` (`AUTH_MODE=dev`), `testing::StaticDirectory` |
| `Mailer` | `send(OutboundEmail) -> provider id` | Resend | Mailpit HTTP API, `testing::RecordingMailer` |

Port futures are `Send` on every target: services call ports inside `ctx.run`, which needs
`Send` futures. The wasm adapters wrap their JS futures and handles in `SendWrapper` (sound
because a Worker isolate is single-threaded). See the `techdepo_ports` crate docs.
