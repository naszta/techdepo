# Customer support: workflows, agents and humans

This is the heart of the demo. Its point is to show **what should be
code, what should be a workflow with an LLM step, what should be an
agent, and when a person has to decide**.

## 0. Decision framework

| Pattern | Use when | Examples here |
|---|---|---|
| **Plain code** | The input is structured and the rule is known. | Email parsing, sender check, order number (regex `TD-\d+`), shipping policy, compatibility, duplicate-charge detection |
| **Workflow** (Restate, durable, code-driven control flow) | The steps are known up front, the decision depends on facts in our systems, there are side effects or money, and it may run for days. | Cancel order, where-is-my-order, refund, investigation |
| **LLM as a function** (one bounded call inside a workflow) | The input is unstructured but the output is a **closed set**. | Ticket classification, "is this reply accepting the refund or the reship?" |
| **Agent** (the LLM chooses the next step from tools) | The goal is open-ended, the number and order of steps depends on intermediate results, tools are read-mostly, and mistakes are cheap and reversible. | Product advice and compatibility questions, general order questions |
| **Human** | The action is irreversible or moves money, confidence is low, the request is a policy exception, or the tone is legal or emotional. | Refund approval, investigation outcome, escalations, reviewing AI replies (per the reply policy) |

The rules of thumb the demo should make visible:

1. **The LLM never decides what code can decide.** Whether an order can
   be cancelled is a function of its state, not a judgement call.
2. **Agents never move money and never send email.** They return an
   *outcome*. Deterministic code acts on it: it sends, applies guardrails,
   and asks for approval.
3. **Tenant scoping comes from code, never from tool arguments.** An
   injected "cancel order TD-9 of another customer" is harmless by
   construction.
4. **Every wait on a person is durable.** Restate awakeables can wait for
   days at no cost.

## 1. Intake

```
support+<org>[.<ticket>]@techdepo.dev
        │ Cloudflare Email Routing (subaddressing enabled, rule → Worker)
        ▼
techdepo-mail-in  (plain code; apps/mail-in, `intake`)
  1. parse MIME (mail-parser: lenient with real-world mail, e.g. raw UTF-8 headers or odd dates,
     which email-message-wire rejects; already in the graph through email-message)
  2. org  = orgs.public_id from the "+tag" → unknown → drop + log (unscoped)
  3. drop + log to the org's email_log: oversized (MAX_EMAIL_BYTES, not even read), malformed,
     auto-replies (Auto-Submitted ≠ no, Precedence: bulk/list/junk, List-Id, X-Autoreply),
     mail from our own domain (and subdomains), no DMARC pass (fail closed: the topmost
     Authentication-Results must come from our MTA, INBOUND_AUTHSERV_ID = mx.cloudflare.net,
     and say dmarc=pass for the From domain; a missing header, another authserv-id, none /
     temperror / permerror or a domain without a policy all drop the mail), more than
     MAX_INBOUND_PER_HOUR accepted messages for the org (mail and the web form together)
  4. sender (header From) must be a member of that org: AUTH_MODE=dev: D1 org_members;
     clerk: Clerk Backend API org memberships, cached ~10 min in KV (MEMBERS_CACHE) and in the
     isolate → not a member → drop + log (no reply: avoid backscatter). Directory down →
     temporary failure (the sending server retries)
  5. ticket = ticket part of "+tag" → else In-Reply-To/References lookup in email_log, then
     ticket_messages → else NEW, ticket_id = TicketId::derive(public org, Message-ID)
     (deterministic ⇒ retries can't fork tickets). Another member's ticket is never
     continued: the message opens its own. A redelivered Message-ID reuses its log entry and
     ticket.
  6. strip quoted history and signatures; email_log keeps the full text (and HTML)
  7. POST {restate}/SupportTicket/{org}:{ticket}/inbound/send
       Idempotency-Key: <Message-ID>
```

Locally, `wrangler dev`'s email trigger runs the handler
(`curl -X POST 'localhost:8789/cdn-cgi/handler/email?from=…&to=…' --data-binary @mail.eml`). It
adds no `Authentication-Results`, so local dev sets `INBOUND_SENDER_AUTH=trust-from` (allowed
only with `AUTH_MODE=dev`): the `From` address is taken as is.
with `AUTH_MODE=dev`, `POST /inbound?from=…&to=…` runs the same pipeline and answers with the
outcome.

- **Web form:** the in-app "Send as email" form posts to techdepo-web. It
  authenticates the user with Clerk instead of DMARC and the membership
  lookup, then makes the same Restate call with `channel = web`. It counts against the same
  per-org `MAX_INBOUND_PER_HOUR` (`429 rate_limited`).
- The customer's **name** handed to the agents is the directory's (Clerk or `org_members`), the
  `From` display name only when the directory has none, and it reaches a system prompt only as a
  plain name (letters, spaces, `-`, `'`, `.`; at most 4 words, 40 characters).
- **Investigations** an admin opens from an order page enter the same
  machinery, but without a ticket.

## 2. `SupportTicket` virtual object: the conversation state machine

The object is keyed by `ticket_id` and holds **no LLM logic**. It reacts
to events and decides where each message goes. Every handler finishes
quickly. Long waits happen in workflows or through awakeables, never by
holding the object's exclusive lock.

**State:**

```
status   : New | Routing | WithRoute{route} | AwaitingCustomer{waiter} | WithHuman | Resolved
messages, classification{intent, probs}, order_number?, assignee: Ai|Human,
waiter?  : { awakeable_id, expects }   // set when a workflow waits for the customer's reply
buffer   : inbound messages that arrived while a route was running
timer_gen: u32                          // generation counter to ignore stale timers
```

**Handlers:**

- **`inbound(msg)`**: append the message, then branch on status:
  - `WithHuman`: refresh the human task ("new customer message") and
    stop.
  - `AwaitingCustomer{waiter}`: resolve the awakeable with the message.
    The waiting workflow interprets it. Only messages received after the
    question was sent answer it (`Waiter::answers_from`); older buffered
    ones are superseded by the question (they stay in the conversation)
    and screened.
  - `WithRoute` or `Routing`: buffer the message. It is processed when
    the route reports back. A message buffered behind a workflow (a refund
    awaiting approval, an investigation awaiting its decision) is
    **screened** at once (`Router/screen`: only `wants_human` and
    `legal_threat`), and either escalates the ticket.
  - Otherwise: set status to `Routing` and one-way send
    `Router/route(ticket snapshot)`.
- **`route_result(outcome)`**: the callback from routes and agents. It
  handles four outcomes:
  - `Reply`: send it, or create a review draft, depending on the AI
    reply policy (§6).
  - `AskCustomer{expects}`: send the question, set `AwaitingCustomer`,
    and arm a reminder timer.
  - `WaitingOnWorkflow`: record the workflow.
  - `Escalate{reason}`: set `WithHuman` and create a human task.

  Afterwards it drains the buffer.
- **`timeout(gen)`**: reminder on the first expiry, then auto-close.
  Durations come from org settings, e.g. 10 minutes in a demo and 3 days
  in "realistic" mode.
- **`human(action)`**: reply, take over, hand back to the AI (optionally
  saying what the customer wants), or close. It also cancels the ticket's pending ticket-owned human tasks
  (escalation, reply review, offers declined): acting on the ticket
  answers them.
- **`task_decided(task, decision)`**: a decision on a ticket-owned human
  task, forwarded by `HumanTasks/decide`. `send` / `edit_and_send` send
  the reviewed draft, `discard` leaves the ticket with a person, and
  `reply` / `hand_back` / `close` do what the matching `human` action
  does. No invocation waits on these tasks, so they have no awakeable.
- **`get` (shared)**: snapshot for the UI.

**As built** (`crates/services/src/support/ticket.rs`, the contract in `support/contract.rs`):

- A **routing generation** replaces the `route` payload of the status: routing a batch of
  messages bumps it, the router and the routes echo it, and a person's action bumps it too, so
  results that arrive after a take-over are ignored. Workflows that report long after their
  route (refund done, the investigation's offer) send no generation and are always applied (a
  person who has the ticket still gets the facts sent, but keeps the status).
- `Router` calls **`classified`** (request-response) before it dispatches: the ticket stores the
  classification, intent and route for the admin's "why" panel and answers whether the
  generation is still current.
- `route_result` also handles `Close` ("thanks!", no reply), `Reroute{intent}` (an agent's
  `reroute`; at most two in a row, then escalate) and `RefundFinished` (from
  `support::refund_finished`: the `refund_done` / `refund_rejected` support templates in the
  ticket's thread; a refund an investigation started also ends the wait on that investigation)
  and `OffersDeclined` (from `Investigation`: the ticket goes to a person with an
  `offers_declined` task).
- An `AskCustomer` without an awakeable is the route's own question: the answer goes back
  through the router, which re-runs the same route when `answers_our_question` says yes.
- A workflow that fails terminally still reports: `RefundWorkflow` closes its refund
  (`rejected`, unless the money already went back) and escalates the ticket
  (`support::refund_failed`); `Investigation` escalates; `Router/route` reports an escalation
  for its generation. The ticket never buffers behind a dead workflow or router.
- A review draft keeps `assignee = ai` with status `WithHuman`; new messages are buffered until
  the draft is decided. An escalation sets `assignee = human`, and the customer is told a
  person will look at it (the `escalated` template, an automatic reply: not when a person
  already had the ticket, not over the rate limit; review drafts are no escalation).
- **"Hand back"** (`hand_back`, optionally with an `intent`) means "AI, answer what is still
  unanswered": the customer messages after our last sent reply are routed again at once, under
  the person's intent when they gave one (forced, `p = 1`: no classification, which is what
  escalated a low-confidence ticket in the first place), otherwise through the classifier
  (escalating again is fine: each round takes a person's click). With nothing unanswered, or on
  a resolved ticket, the status becomes `New` and handling resumes with the customer's next
  message. Either way a workflow still waiting for the customer is told it gets no answer
  (`withdrawn`), like any other action of a person. `other` is no intent to hand back with.
- A new message while a person has the ticket refreshes their pending tasks
  (`HumanTasks/refresh`: summary "New customer message: …", and an escalation's
  `new_customer_message`).
- Outbound mail is rendered by the ticket (so `ticket_messages` stores exactly what is sent) and
  queued with `Notifications/ticket` one-way. At most 6 automatic replies per ticket and hour;
  over that, or when guardrails failed, the ticket escalates instead of sending.

**Why a virtual object and not a workflow:**

- A ticket is a long-lived *entity* that receives an unknown number of
  events.
- Restate serializes those events per ticket for free, which closes
  races such as two emails arriving at once.
- The linear processes it kicks off (refund, investigation) *are*
  workflows.

## 3. Router: classification and routing

`Router/route` is a service. It runs three steps.

**1. Deterministic extraction.** Order numbers come from a regex, and
mentions of the customer's orders are matched against D1.

**2. Classification.** One Clef call answers every question in a single
pass:

| Question | Type | Options / use |
|---|---|---|
| intent | choice | `cancel_order`, `order_status`, `billing_problem`, `refund_or_return`, `product_advice`, `order_question`, `other` |
| contains_request | noul | "thanks!" → close the ticket without replying |
| wants_human / legal_threat | noul | yes → escalate |
| answers_our_question | noul | used when the ticket was waiting on a missing slot |

**3. Decision.** Escalate if `p(top intent)` is below the org threshold
(default **0.70**, configurable in admin settings) or the margin over the
runner-up is below 0.15. Otherwise dispatch:

| Intent | Route | Kind |
|---|---|---|
| `cancel_order` | `CancelOrder` | workflow |
| `order_status` | `WhereIsMyOrder` → maybe `Investigation` | workflow and policy |
| `billing_problem` | `BillingIssue` | workflow and deterministic detectors |
| `refund_or_return` | `RefundRequest` → `RefundWorkflow` | workflow and human |
| `product_advice` | `ProductAdvisor` | **agent** |
| `order_question` | `OrderAssistant` | **agent** (read-only) |
| `other` | none | human |

The probabilities and the chosen route are stored on the ticket and shown
in admin, so people can see *why* a ticket went where it did.

**Routes are services invoked by name:** `<Route>/run(RouteRequest)`, reporting exactly one
`RouteOutcome` to `SupportTicket/route_result` (`support::contract`). The request carries the
customer's own orders from D1, the explicitly named one (`order_explicit`), the numbers that are
*not* the customer's (never acted on), the conversation, and `resumed` when it continues the
route's own question. `contract::route_target` lists the deployed routes; the router escalates
a route that is not deployed. Each classification is stored as a `router` trace (`agent_runs`,
one `classification` step); a classifier that keeps failing (4 attempts) escalates.

**Missing information.** Read-only routes may infer the order when the
customer has exactly one candidate order, and the reply says which order
was used. Routes that **act** on an order always require an explicit
order number. When one is needed and missing, the route returns
`AskCustomer{expects: order_number}`. That email includes a link to
`/account/orders`. When the customer replies, the ticket re-runs **the
same route**, provided the "answers our question" check says yes.
Otherwise the reply is re-triaged.

## 4. Routes

### `CancelOrder` (workflow, no LLM)

Order state decides everything:

| Order state | Action | Reply (template) |
|---|---|---|
| `awaiting_payment` | `Order.cancel` | "Cancelled" (ticket resolved) |
| paid, not shipped | start `RefundWorkflow` (needs approval) | "Refund requested, awaiting confirmation" |
| shipped, in transit | none | "Already shipped. Once it arrives you can return it within 14 days." |
| delivered, within 14 days | `Order.request_return` (status `return_requested`) | "Return requested. Here's how to send it back; the refund starts once we receive it." |
| delivered, after 14 days | none | "The return window has closed" (warranty pointer) |
| return requested / returned | none | "Your return is under way" |
| cancelled / refunded | none | "Already cancelled or refunded" |

The state comes from `domain::OrderStatus::cancellation`, the 14 days from
`domain::within_return_window` on the order's simulated calendar.
`Order.request_return` is the internal handler behind the
`request_return` transition (the demo-controls button calls the same
transition through `Order/demo_action`).

### `WhereIsMyOrder` (workflow and policy function)

- Not paid: "awaiting payment".
- Not shipped: "being prepared".
- Delivered: "the carrier reports delivery on X". If the customer
  insists, escalate.
- Shipped: `domain::shipping_policy(workdays_since_ship(order clock))`:
  - **0–3 workdays:** the standard policy (1–3 workdays) with the ETA.
  - **4:** "the carrier is overloaded, please give it a little longer".
  - **5 or more:** start `Investigation`, reply "we've opened an
    investigation", and return `WaitingOnWorkflow`.

### `BillingIssue` (workflow and deterministic detectors)

- **Duplicate charge:** two captured charges with the same amount within
  a short window. If found, start `RefundWorkflow` for the duplicate with
  the evidence attached, which still needs approval, and reply.
- **Invoice without billing address:** `Invoicing.void_and_reissue`. This
  is automatic because no money moves. Reply with the new invoice.
- **Neither detector matches:** escalate. Customers aren't told
  something is wrong when the data says it isn't.

### `RefundRequest`

Validate the order state with the same table as `CancelOrder`. If the
order is refundable, start `RefundWorkflow` and reply. If it is delivered
and within the return window, request the return the same way, so the
order shows **Return requested** as the returns policy promises; the
refund starts when the parcel is back (`returned`). Otherwise explain
why.

**As built** (`crates/services/src/support/routes/`): each route is a private Restate service
(`CancelOrder`, `WhereIsMyOrder`, `BillingIssue`, `RefundRequest`) with one `run(RouteRequest)`
handler that reads its facts from D1 in one journaled step (`load-order`, `detect-billing`),
decides with a pure table (`cancel::order_plan`, `wismo::wismo_plan`, the duplicate detector),
acts, and reports one outcome with the request's generation.

- **Order choice** (`routes::choose_order`): `CancelOrder`, `RefundRequest` and `BillingIssue` act,
  so they need an order named in the new message (`order_explicit`); `WhereIsMyOrder` also takes
  the ticket's order or the customer's only one (the reply says "we looked up TD-…"). Otherwise
  `AskCustomer{expects: order_number}` (`ask_order_number`, with the `/account/orders` link, or
  `order_not_found` naming the numbers that are not the customer's). A second answer that still
  names none of the customer's orders escalates.
- **Refunds a route starts** (cancel or refund of a paid, unshipped order; a duplicate charge)
  are planned first with `RefundWorkflow`'s own `plan_refund`: a refund already under way is
  answered with `refund_under_way` instead of a second workflow. The refund ID derives from the
  ticket, generation and order, so a retried route finds the same workflow. The route reports
  `WaitingOnWorkflow{refund}` with the reply; the refund's result email resolves the ticket.
- **`WhereIsMyOrder`**: the workdays come from the shipment's ship date and the order's simulated
  today (D1 `order_clock`). An open investigation is answered with `investigation_under_way`
  (it reports to the ticket that opened it). A delivered order asked about again after our
  "delivered on X" answer in the same conversation escalates. Cancelled, refunded and returned
  orders get "nothing is on its way".
- **`BillingIssue`**: both detectors run; when both match, the reply (`billing_fixed`) covers the
  duplicate's refund and the corrected invoice. `Invoicing/void_and_reissue` emails the corrected
  invoice (attached) itself; the ticket's reply names it.
- A route that fails terminally reports an escalation, so the ticket never waits on a dead route.
  `RefundWorkflow` re-plans the refund itself, so a refund planned by a route (or accepted in an
  investigation) can still be refused there (time of check vs time of use); that ends in an
  escalation too (below).

### `RefundWorkflow` (Restate workflow, keyed by refund ID)

```
run(req{order, amount, reason, evidence, pre_approved, ticket?}):
  ctx.run  insert refund (requested)
  if !pre_approved:
      task = HumanTasks.create(approve_refund, evidence, decisions=[approve, reject{reason}])
      decision = await awakeable  (+ SLA: select with ctx.sleep → nudge admin, keep waiting)
      if reject → SupportTicket.route_result(Reply(template refund_rejected + reason)) → end
  ctx.run  payments.refund(order, amount)   idempotency key = refund id
  ctx.run  credit note if invoiced, unless reason = duplicate_charge (never invoiced)
  Order.mark_refunded (full) | Order.record_event (partial / duplicate)
  SupportTicket.route_result(Reply(template refund_done)) → ticket resolved
on a terminal failure (re-planned: nothing left or a refund already in progress; any failed call):
  record-error (refund rejected, refund_pending cleared) unless the money went back
  SupportTicket.route_result(Escalate) (`support::refund_failed`) → the ticket goes to a person
```

### `Investigation` (Restate workflow, keyed by investigation ID)

```
run(order, ticket?):
  ctx.run  insert investigation; Order.flag(investigation_open)
  ctx.run carrier.open_trace(tracking)                 // retried if the carrier API is "down"
  trace = Poller(every 30s, timeout 5m).poll(track)    // restate-ext durable polling until the trace concludes
  if trace == resurfaces → reply "found, arriving <next workday>"; resolve → end
  // no information
  decision = HumanTask(investigation_decision, decisions=[offer_refund, offer_reship, offer_both, custom_reply])
  send offer email (template)
  loop (max 2 rounds):
      reply = await customer awakeable (registered as the ticket's waiter; timeout → reminder → escalate)
      verdict = Clef choice {accept_refund, accept_reship, decline, unclear}
      if p < threshold or unclear → round 1: ask to clarify; round 2: escalate
      accept_refund → RefundWorkflow(pre_approved = true)   // a human already made the offer
      accept_reship → Order.place_replacement(original) → paid by store (payment_method = store, no payment), ships as normal
      decline       → HumanTask(escalation: "customer declined all offers") → end
```

**As built** (`crates/services/src/investigation.rs`):

- **The trace takes a moment:** `Carrier/open_trace` opens the trace on its first call (`open`)
  and answers once `carrier::TRACE_LATENCY` (15 s of real time) has passed. The workflow polls it
  with `restate_ext::poll::Poller` (every 5 s, timeout 5 min); no answer in time counts as "no
  information" (the task says the carrier did not answer).
- **Records:** the workflow is the only writer of its `investigations` row (`open` → `tracing` →
  `awaiting_decision` → `awaiting_customer` with `rounds` → `resolved`/`escalated`, with
  `carrier_result`, `decision`, `outcome`, task, refund, replacement), and flags the order
  (`investigation_open`) through `Order/record_event` when it opens and closes.
- **Reporting:** to the ticket with `SupportTicket/route_result` without a generation:
  `ParcelFound` (resolve), the offer as `AskCustomer{expects: offer_reply, awakeable}` (round 2:
  `offer_unclear`), `ReplacementPlaced` (resolve), `Escalate` (no reply after the reminder, two
  unclear replies, replacement out of stock), and the new `OffersDeclined{investigation, last
  reply}`, for which the ticket creates the `offers_declined` task (ticket-owned, like
  escalations). A `withdrawn` answer (a person took the ticket) closes the investigation as
  escalated without a report.
- **Accepting the refund:** the workflow first plans it (`plan-refund`): nothing left to refund
  (an admin refunded the order while the offer was out) or a refund already under way escalates
  instead. Otherwise it reports `WaitingOnWorkflow{refund}`, runs the pre-approved
  `RefundWorkflow` (reason `lost_parcel`, ID derived from the investigation) and **awaits** it:
  the investigation closes as `refunded` only when the refund completed (a refused payment or a
  failed refund closes it as escalated, and the ticket goes to a person). The refund's result
  email resolves the ticket, and a refund that belongs to an investigation also ends the
  ticket's wait on that investigation.
- **Accepting the replacement:** `Order/place` of a copy of the order (`payment_method = store`,
  free shipping, `replacement_for`, no faults; ID derived from the investigation), a
  `replacement_created` entry on the original order, then `replacement_placed`.
- **Reply interpretation** (`offer_step`, pure): one classifier pass with the choice
  `{accept_refund, accept_reship, decline, unclear}` plus the router's screen (`wants_human`,
  `legal_threat`; either escalates at once instead of acting or asking again, `reply_step`; a
  decline still gets its `offers_declined` task, which reaches a person anyway); an accepted
  offer that was made, or a decline, at or above the org's escalation threshold acts; anything else is asked
  again once, then escalated. Each interpretation is a `reply_interpreter` trace (ticket and
  investigation). Only a reply written after the offer went out is an answer to it (support.md
  §2, `answers_from`).
- **Without a ticket** (opened by an admin) nobody can be asked: `offer_refund` and
  `offer_reship` are carried out right away, `offer_both` closes as escalated (contact the
  customer), and `custom_reply` closes with the reply unsent.
- A terminal failure escalates the ticket and closes the investigation as escalated.

- **Use a fresh awakeable for each conversation turn.** Workflow durable
  promises resolve only once, and a conversation can take several turns.
- **The carrier call is a fixed workflow step, not an agent tool call.**
  There is nothing for an LLM to decide. The same carrier functions *are*
  exposed as tools to `OrderAssistant`, which shows the same capability
  used both ways.

## 5. Agents

### Minimal agent loop (`crates/agent`)

```rust
pub trait Tool<Cx: ?Sized> {                            // Cx: the caller's per-run context
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    type Args: DeserializeOwned + JsonSchema;          // schema sent to the model (schemars, inlined)
    async fn call(&self, cx: &Cx, args: Self::Args) -> Result<serde_json::Value, ToolError>;
}
// in crates/services (agents::tools): struct ToolCx<'a, R: Runtime> { rt: &'a R, db, scope: OrgScope, customer: UserId, ticket: TicketId }

pub trait Runtime {                                     // one journaled step; services: ctx.run(..).name(name)
    async fn step<T: Serialize + DeserializeOwned>(&self, name: &str, step: StepFn<T>) -> Result<T, StepError>;
}

pub type Outcome = api_types::support::AgentOutcome;   // Reply | AskCustomer{question, expects} | Escalate | Reroute

pub async fn run<R: Runtime, Cx>(rt: &R, env: &AgentEnv /* llm + budget */, agent: &AgentDef,
                                 tools: &ToolSet<Cx>, cx: &Cx, conversation: Vec<ChatMessage>) -> AgentRun {
    for step in 0..agent.max_steps {
        // one journaled step `llm-<n>`: budget check → llm.chat → record usage.
        // On retry or replay the model is NOT called again, so the cost is paid once.
        let resp = rt.step(&format!("llm-{n}"), ..).await;  // failure / budget spent → Escalate
        trace.push(..);
        if resp.tool_calls.is_empty() { return reply(resp.text); }   // empty or cut off → Escalate
        for call in resp.tool_calls {
            if let Some(o) = terminal(&call) { return o; }   // ask_customer / escalate_to_human / reroute
            let out = tools.dispatch(cx, &call).await;       // errors go back to the model as tool results;
            messages.push(tool_result(call.id, out));        // ToolError::Fatal → Escalate
        }
    }
    escalate("step limit")
}
```

- **Durability:** each LLM call is one journaled step (`ctx.run` through the
  services' `Runtime` impl). `crates/agent` does not depend on Restate, so
  the loop is tested natively (`Inline` runtime, `ports::testing::ScriptedLlm`).
  - Read-only tools run inside `ctx.run` themselves (their `Cx` holds the
    context), so their results are journaled.
  - Pure tools such as `compat` run without one, because they are
    deterministic.
  - There are no side-effecting tools in v1.
- **Never fails:** model failures after retries, a spent budget, a fatal
  tool error, an empty or truncated answer and the step limit all end in
  `Escalate`, with the reason in the trace.
- **Terminal outcomes are tools:**
  - `ask_customer`, `escalate_to_human` and `reroute` are tools.
  - A plain-text answer means `Reply`.
  - Workers AI takes no `tool_choice` (allama's Workers AI adapter
    sends none), so plain text is also the natural way to finish.
- **Budget:** max steps (6), plus a per-org daily token budget checked
  before every LLM call (`techdepo_llm::UsageLedger::for_org`, the loop's
  `Budget` hook; `llm_usage`).
- **Trace:** every model call (prompt digest, the messages new since the
  previous call, the response, tokens, latency) and tool call (arguments,
  result or error) is a `TraceStep`, written to `agent_steps` by the caller.
- **Guardrail retry:** append the violation as a user message to
  `AgentRun::messages` and call `run` again; step names continue
  (`llm-<n>` counts the conversation's model turns).
- **Model access:** behind the `llm` facade, so the provider (allama) can be
  swapped without touching the loop. allama deliberately has no agent
  loop: it declares and parses tools, and `crates/agent` runs them.
- **Offline (`LLM_BACKEND=mock`):** `techdepo_llm::MockLlm` plays both agents
  from the conversation and the tool results. It calls the tools by the
  names below and finds their parameters in the schemas (`text`,
  `product_ids`, `parts` / `replace_category` / `price_band`, `id`,
  `order_number`, `slug`); its module docs list the result shapes it reads.

### `ProductAdvisor` agent

- **Goal:** answer product and compatibility questions, and recommend
  alternatives.
- **Tools:**
  - `search_products(text?, category?, socket?, memory_type?, form_factor?, price_min?, price_max?, sort?)`:
    typed filters plus simple name and tag matching over the in-memory
    catalog. It returns compact rows (id, name, price, key specs). With
    about 90 products there is nothing for a vector index to add.
  - `get_product(id)`: specs plus the product's rich content.
  - `check_compatibility(product_ids)`: the **compat engine report**.
    The model is told it must call this before stating anything about
    compatibility.
  - `find_alternatives(parts, replace_category, price_band)`: calls
    `compat::alternatives`.
  - `get_my_builds()`: the customer's saved builds.
  - `list_guides()`, then `read_guide(slug)`: buying guides.
  - `ask_customer`, `escalate_to_human`, `reroute`.

### `OrderAssistant` agent (read-only)

- **Goal:** questions about orders and policies that are not covered by
  a workflow. Examples: "can I change the delivery address?", "do you
  ship to Austria?", "what's on my invoice?"
- **Tools:**
  - `list_my_orders()`
  - `get_order(order_number)`
  - `get_tracking(order_number)`
  - `get_invoices(order_number)`
  - `list_policies()`, then `read_policy(slug)`
  - the terminal tools
- **No write tools.** If the customer wants something *done*, the agent
  calls `reroute(intent)`, and the deterministic route takes over.

### Guardrails, in code, after the agent returns `Reply`

1. **Grounding:** every product name, SKU and price in the reply must
   exist in the catalog with the correct price. Names are found with
   `Catalog::product_mentions`: a full name or a prefix only one product
   has ("Lumen Apex") passes, a prefix of several ("Halcyon HX 7070")
   fails. Content and knowledge-base text only use names that pass
   (`xtask catalog validate` checks), so quoting them verbatim is safe.
2. **Compatibility claims:** if the reply asserts compatibility, the
   trace must contain a `check_compatibility` call covering those parts,
   with a matching verdict.
3. **Format:** length, no internal IDs, and links only to our own
   domain.

On failure the agent gets one retry with the violation as feedback.
After that the ticket is escalated. Guardrail results are stored in the
trace.

**As built** (`crates/services/src/agents/`):

- **Routes:** `ProductAdvisor` and `OrderAssistant` are private Restate services with one
  `run(RouteRequest)` handler (`contract::route_target`), like the deterministic routes. The
  handler loads the org settings, runs `agents::pipeline::answer` and reports one
  `RouteOutcome` with the deciding trace's `agent_run_id`: `Reply` / `AskCustomer` carry the text
  as `Reply::Ai(AiReply)` (guardrails, judge, `p(intent)`), `Escalate` gets the agent's name in
  its reason, and `Reroute` goes to the ticket unless it names the agent's own route or `other`
  (then it escalates instead of looping).
- **Pipeline** (`pipeline.rs`, generic over the loop's `Runtime`, so it runs natively in tests with
  `Inline` over SQLite): agent → guardrails on the AI-authored text (a reply *or* a question) →
  at most one retry (a second `agent_runs` row, `attempt = 2`, the conversation continued with the
  violations as a user message) → judge on text that passed → one `record-trace` batch. The
  guardrails and the judge see the tool calls of *every* attempt so far: the retry continues the
  conversation, so the model may rely on what the first attempt's tools returned.
  `runtime::Journaled` is `ctx.run(..).name(..)` with a bounded retry policy (5 attempts, backoff
  from 0.5 s): a model that stays down escalates instead of retrying forever.
- **Tools** (`tools/`): `ToolCx<'a, R>` holds the org scope, customer and ticket from the request
  (never from arguments) and the runtime; D1 reads run through `ToolCx::read`, a journaled step
  `tool-<n>-<tool>` that journals the JSON the model sees. Pure tools (catalog, compat, knowledge
  base) run directly. Parameters as the mock model expects them: `search_products(text,
  category, socket, memory_type, form_factor, price_min, price_max` in euros`, sort, limit)`,
  `get_product(id)`, `check_compatibility(product_ids)` (two parts: `compat::check_pair`; more:
  `check_build` without the "missing part" rules), `find_alternatives(parts, replace_category,
  price_band: similar | cheaper | any)` (±25 % for `similar`, top 3 with caveats),
  `get_my_builds()`, `list_guides()` / `read_guide(slug)`, `list_my_orders()`,
  `get_order(order_number)`, `get_tracking(order_number)`, `get_invoices(order_number)`,
  `list_policies()` / `read_policy(slug)`. Another customer's order is "not found", worded so the
  model never speculates about it. Prices are given as text (`€69.90`), exactly as replies must
  quote them.
- **Guardrails** (`guardrails.rs`, pure): *grounding*: product mentions resolve to one product, a
  capitalised word right after a name ("Kestrel B650 Turbo") is a product we do not sell, the
  price right after a name is its catalog price, every other price is one a tool returned, SKUs
  exist; *compatibility claims*: positive or negative compatibility wording needs a successful
  `check_compatibility` whose parts cover every product named (or a `find_alternatives`
  candidate), with a matching verdict; *format*: 1–2000 characters, no internal IDs (`ord_…`,
  product slugs, …), no placeholders, links only to the app's and the support domain: URLs,
  bare domains (`example.com/verify`) and email addresses alike, since mail clients link them.
- **Loop details:** when a terminal call (`ask_customer`, `escalate_to_human`, `reroute`) is not
  the last call of a response, the later calls are not run but are traced and answered as
  skipped (chat APIs reject unanswered tool calls); `llm_calls` counts answered model calls only
  (not one the budget refused).
- **Trace:** every attempt is an `agent_runs` row with its LLM and tool steps plus one
  `guardrail` step per guardrail; the deciding attempt carries the confidence breakdown. The
  judge's pass is its own `judge` run (one `classification` step) with its `llm_usage`. The
  router's and the reply interpreter's classifier tokens are recorded in `llm_usage` too
  (recorded, not gated). Run IDs derive from ticket, generation, agent and attempt, so a retried
  invocation upserts the same rows.
- **Budget:** `Config::llm_budget` (`LLM_DAILY_TOKEN_BUDGET_*`) feeds
  `UsageLedger::for_org(org, today)`; a spent budget escalates with the reason in the trace
  (status `failed`).

## 6. Outbound email

**Agents never send email themselves; the `Notifications` service sends
it.** Agents and routes return an *outcome*, and `SupportTicket` calls
`Notifications.send_ticket_reply`, which calls `Email/send` (our durable
service over the `Mailer` port, architecture §7). Reasons:

- One choke point for guardrails, the reply policy, rate limits and the
  email log.
- Threading headers are always right, and retries and idempotency come
  from Restate rather than from the model.
- It is easier to explain in the demo: "the agent decides *what* to say,
  the system decides *whether and how* to send it".

### AI reply policy (org setting)

The policy applies to **AI-authored text**: agent replies and
`ask_customer` questions. Workflow emails come from code-authored
templates, so they are always sent.

| Setting | Behaviour |
|---|---|
| `always_send` | Send as soon as the guardrails pass. |
| `always_review` | Every AI-authored email becomes a **review draft** human task: approve, edit and send, or discard and take over. |
| `send_above_confidence` (threshold, default 0.80) | Send automatically if the reply confidence is at or above the threshold. Otherwise create a review draft. |

**Reply confidence** is computed after the guardrails pass, as
`min(p(intent) from triage, judge score)`.

- The **judge** is one Clef pass over the customer message, the
  tool-result summary and the draft. It asks three yes/no questions:
  - does the draft answer the question?
  - is every claim supported by the tool results?
  - is the tone appropriate?
- The judge score is the lowest of the three P(yes) values.

The breakdown is stored in the trace and shown on the review task. In a
demo, moving the threshold slider shows the same email being sent
automatically or held for review.

As built, the judge also scores an agent's `ask_customer` question (its first question reads
"does the draft answer the question, or ask only for what is needed to answer it?"), so a
question can be sent automatically too. Without a judge result (the classifier failed) the
confidence is 0, which never sends automatically under `send_above_confidence`. The customer's
message is fenced in the judge's text and marked as untrusted data, so a message that addresses
"the grader" is not an instruction to it.

**Guardrail failures never send, whatever the setting.** The agent gets
one retry, then the ticket is escalated (§5).

**Message format:**

- Transactional replies use templates (askama): cancel, refund started,
  refund done or rejected, investigation opened, offer, reminder, and
  so on.
- Agent replies are inserted into the standard layout.

**Threading headers:**

| Header | Value |
|---|---|
| `From` | `TechDepó Support <support+<org>@techdepo.dev>` |
| `Reply-To` | `support+<org>.<ticket>@techdepo.dev`. The ticket ID is in the address, which is the primary way the ticket is identified. |
| `Message-ID` | `<<ticket>.<n>@techdepo.dev>` |
| `In-Reply-To` / `References` | the customer's message IDs. The fallback for identifying the ticket. |
| `Subject` | `Re: … [#<ticket>]`. A third fallback, and readable for people. |

## 7. Knowledge without a vector database

The catalog has about 90 products, and there are about 12 short policy
and guide documents. At that size, **tools beat embeddings**: retrieval
is explicit, deterministic, visible in the agent trace, and needs no
ingestion pipeline.

- **Products:** `search_products` uses typed filters over the catalog
  compiled into the services worker. `get_product` returns specs and the
  rich content.
- **Guides and policies:** `data/kb/*.md` with front-matter (`slug`,
  `title`, `summary`), also compiled in (`agents::kb`, `include_str!`; a test fails when a file
  is not listed). `guide-*` documents are guides (`ProductAdvisor`), the rest policies
  (`OrderAssistant`).
  - `list_*` returns slugs, titles and summaries.
  - `read_*` returns one document.
  - The model chooses what to read, which is "agentic retrieval".
- **Short, stable policy facts** go straight into the `OrderAssistant`
  system prompt, e.g. "shipping takes 1–3 workdays" and "returns are
  accepted within 14 days". The documents are for detail.
- **Revisit** Vectorize or AI Search only if the catalog grows by an
  order of magnitude, or free-text product discovery becomes poor in the
  evals. A chunking and embedding ingestion workflow would then be a
  natural Restate showcase.

## 8. Human tasks and escalation

**`HumanTasks` service:**

- `create(kind, context, allowed_decisions, awakeable_id?)` writes
  `human_tasks`. Only the kinds a workflow waits on carry an awakeable:
  `approve_refund` (`RefundWorkflow`) and `investigation_decision`
  (`Investigation`). The ticket-owned kinds, `review_reply` and
  `escalation` from `SupportTicket` and `offers_declined` from the
  investigation's last step, have none, because nothing waits on them
  (`HumanTaskKind::has_waiter`).
- `decide(task_id, decision, note)` is called by the admin UI through
  techdepo-web. It validates the decision against the allowed set and
  records who decided. Then it resolves the awakeable or, for a
  ticket-owned task, calls `SupportTicket/task_decided`. It is the only
  path that resolves an awakeable, so the browser never sees awakeable
  IDs.

**Escalation triggers:**

| Source | Trigger |
|---|---|
| Classifier | top-intent probability below the threshold, small margin, `other`, wants a human, legal threat |
| Screen | wants a human or legal threat in a message buffered behind a workflow, superseded by a question, or replying to the lost-parcel offer |
| Agent | `escalate_to_human` tool, step limit, guardrail failed twice, LLM or tool error after retries, budget exhausted |
| Workflow | customer declines all offers, two unclear replies, no reply after the reminder, a refund or investigation that failed terminally, an accepted refund that cannot start or be paid |

**When a person takes over** (`assignee = Human`), the AI stops handling
that ticket. New messages update the human task (`HumanTasks/refresh`). "Hand back to AI"
resumes automatic handling: it routes the messages nobody answered yet, under the intent the
person picked (the escalation task and the ticket page offer one), see §2.

**The customer hears that a person took over.** Whenever the ticket escalates while the AI had
it, one `escalated` template ("a colleague will look at this") goes out, within the automatic
reply limit. Code-authored, so no guardrails; the person's own reply follows later.

**Refund approvals are approvals, not escalations.** They always happen,
whatever the confidence.

## 9. Safety, cost, abuse

- **Emails are untrusted input.** That includes prompt injection, so:
  - tools are tenant-scoped by code
  - agents have no write tools
  - money always needs approval
  - guardrails run before anything is sent
- **Loop protection:** ignore auto-submitted mail, and send at most N
  automatic replies per ticket per hour.
- **Cost:**
  - per-org daily LLM token budget (`llm_usage`)
  - per-org inbound email rate limit
  - a global daily budget
  - sign-up is controlled in Clerk (open during the demo period,
    restricted otherwise), so the app has no sign-up gate of its own
  - optional automatic expiry of demo orgs after N days
    (`ORG_EXPIRY_DAYS` on the services worker, off by default, 30 in
    production): provisioning stamps `orgs.expires_at` and schedules a
    delayed `Org/expire`, a durable Restate timer rather than a cron. It
    deletes the org and all its data in one batch; the next sign-in gets a
    fresh shop with a new support address. The deadline is fixed at
    provisioning (a later change of the setting moves none; turning it off
    spares orgs whose timer fires afterwards), and orgs provisioned while
    it was off never expire. Admin → Settings shows the date.
- **Privacy:** prompts and traces contain customer emails. Traces are
  visible only to the org's own admins.

## 10. Evals (`evals/`, `xtask eval`)

- **Classification set:** about 60 realistic emails, including ambiguous
  ones, multi-intent ones, very short ones, angry ones, prompt-injection
  attempts and a few non-English ones.
  - Each has an expected intent and expected escalation.
  - The report covers accuracy, a confusion matrix, and **calibration**
    (accuracy per probability bucket). Calibration is how the 0.70
    threshold gets chosen rather than guessed.
- **Agent scenarios:** seeded build or catalog questions with assertions:
  - a `check_compatibility` call is required
  - recommended products exist and fall in the price band
  - no hallucinated SKUs
  - the judge's reply confidence is recorded, to calibrate the
    `send_above_confidence` default
  - correct escalation on out-of-scope questions
- Run on demand, and also when the model or prompts change.

**As built** (`xtask/src/eval/`, reports in `evals/reports/`):

- **`cargo run -p xtask -- eval classify|agents [--backend mock|workers-ai]`.** `mock` (default)
  uses the offline models; `workers-ai` calls Workers AI over HTTP (chat:
  allama-openai-compat's `Cloudflare` preset; Clef: allama-systemone's) with
  `CLOUDFLARE_ACCOUNT_ID` / `CLOUDFLARE_API_TOKEN` from the environment or `.env`, configured
  like the services worker (`AI_AGENT_MODEL`, `AI_CLASSIFIER_MODEL`, `AI_FALLBACK_CLASSIFIER`;
  `--agent-model` / `--classifier-model` override them to compare models). Retryable model
  errors are retried (5 attempts), as Restate would. Each run writes
  `evals/reports/<eval>-<backend[-model]>.md` and fails below the eval file's `minimums`.
- **Classification** (`evals/classification.yaml`, 61 cases tagged `clear`, `short`, `multi`,
  `vague`, `angry`, `human`, `legal`, `injection`, `non_english`, `thanks`, `off_topic`): each
  case is classified exactly as the router classifies a ticket's first message
  (`router::triage_questions`, `Triage::read`) and decided with `router::decide` on the default
  settings. `intent` and `outcome` (`route` / `escalate` / `close`) may list several acceptable
  values. Metrics: intent accuracy, decision accuracy, escalation recall, **wrong-route rate**
  (dispatched for an intent the customer did not express; dispatching one of several intents
  counts against escalation recall instead), automation; per tag; the confusion matrix;
  calibration over the cases whose top intent has a route (`other` escalates at any
  probability) with the expected calibration error; a threshold sweep (0.40–0.95) and a
  **suggested threshold**: the most correct decisions within the wrong-route and recall limits,
  the middle of the thresholds that tie. Minimums: intent 80 %, decision 85 %, escalation
  recall 90 %, wrong route ≤ 5 %. The mock's suggestion (0.80) reflects its keyword scores
  only; the 0.70 default is to be set from a Clef run (not done yet: there is no Cloudflare
  account). Each answer's model is checked: answers from the JSON-mode fallback are counted and
  left out of the calibration.
- **Follow-ups and offer replies** (same file, `follow_ups` / `offer_replies`,
  `xtask/src/eval/replies.rs`): replies while a route's question is pending, classified with
  `answers_our_question` and decided with `router::decide` (`resume` / `route` / `escalate` /
  `close`; demo scenario 7), and replies to the lost-parcel offer, read as `Investigation` reads
  them (`investigation::reply_questions`, `reply_step` in round 1: `refund` / `reship` /
  `declined` / `clarify` / `escalate`; demo scenarios 4 and 5). Minimums: 85 % each, and **no
  wrong action** (a refund or replacement the customer did not choose).
- **Agents** (`evals/agent_scenarios.yaml`, 20 scenarios): each runs `agents::pipeline::answer`
  (`Inline` runtime) in a fresh seeded SQLite shop (catalog in stock, the customer with one
  order `{my_order}`, another customer's order `{other_order}`), `p(intent)` = 1.0 so the reply
  confidence is the judge's score. Assertions: outcome (and reroute intent), tools called
  successfully (e.g. `check_compatibility`), phrases, and **recommendations**: catalog products
  the reply names (except the customer's own, `given`) matching a filter (category, spec
  fields), at least *n*, none matching `avoid`, within ±*pct* % of a named reference product
  and/or a euro budget. Every AI text must also pass the pipeline's guardrails, name only
  unambiguous catalog products and only catalog SKUs; a failed agent run (model down, budget,
  step limit) never passes. `critical` scenarios (demo scenario 8, injection, another
  customer's order, reroutes, a request for a person) must all pass; minimum pass rate 80 %.
  The report records the judge's score of every sendable text and sweeps
  `send_above_confidence` thresholds (good texts sent vs. bad ones held), suggesting the
  lowest that holds every bad text while still sending good ones. On `mock` this sweep measures
  nothing (the offline judge scores every grounded draft alike) and the report says so.
