# TechDepó: plan

TechDepó is a demo webshop for PC parts. It has excellent product
information, a compatibility checker that is always correct, and a PC
builder. Underneath, it is a showcase for **what should be code, what
should be a durable workflow, what should be an AI agent, and when a
person must decide**. It is built in Rust on Cloudflare Workers and
Restate.

Each Clerk organization gets its own isolated shop instance. After
placing an order, a user can move it through its lifecycle and inject
faults (double charge, invoice without an address, lost parcel), then
email support and watch workflows, agents and human approvals handle it.

| Document | Contents |
|---|---|
| [architecture.md](architecture.md) | Deployables, Restate on Workers, workspace layout, data model, auth, AI services, email, local dev, CI, risks |
| [commerce.md](commerce.md) | Tenancy, catalog dataset, compatibility engine, storefront and builder, order lifecycle, fault injection, simulated time, invoices, both dashboards |
| [support.md](support.md) | Agent vs workflow framework, email intake, ticket state machine, classification and routing, every route and workflow, agent loop and tools, guardrails, outbound email and the AI reply policy, knowledge via tools, human tasks, safety, evals |
| [design.md](design.md) | UI design: principles, daisyUI theme, status vocabulary, layouts, key screens, demo controls style |
| [setup.md](setup.md) | Points to [../deploy.md](../deploy.md) (the two deployments, account setup, secrets) and [../local-development.md](../local-development.md) |
| [roadmap.md](roadmap.md) | Milestones M0–M7 and the demo scenarios (also the acceptance tests) |
| [../presenter-guide.md](../presenter-guide.md) | Not part of the spec: how to present the demo, scenario by scenario |

## Key decisions

| Decision | Why | Alternatives considered |
|---|---|---|
| Dioxus **CSR SPA** on Workers Static Assets, plus an axum `/api/*` on workers-rs | Dioxus fullstack doesn't compile for Workers yet, and the dioxus-clerk demo already uses this pattern | Fullstack or SSR (blocked); a separate API worker (unnecessary) |
| Three workers: `web`, `mail-in`, `services` (a single Restate deployment) | Clear separation: edge adapters vs durable logic. The services worker can be split later without changing callers. | One worker per service (premature) |
| **Reads from D1, every write through Restate** | Every mutation is journaled, retried and observable, and the UI stays simple | Writes from the web worker straight to D1 |
| `Order` is a **virtual object** (state machine). Refund and investigation are **workflows**. A ticket is a **virtual object**. | Entities that receive arbitrary events are VOs. Linear, long-running processes with human waits are workflows. | Making the order one big workflow (awkward for off-path events such as refunds or cancellations at any point) |
| Classification with **Workers AI Clef** | It returns a probability for each option in one pass, which gives a real confidence for the escalation threshold | Self-reported LLM confidence (poorly calibrated, kept as fallback) |
| Agents **only** for product advice and open-ended order questions, with read-only tools | Those are the only places where the control flow can't be known up front | An agent for every intent (riskier and less explainable) |
| Agents return outcomes. **Code sends the email** through `Notifications` → our `Email/send` service, gated by a per-org **AI reply policy**: `always_send`, `always_review`, or `send_above_confidence` | One place for guardrails, the reply policy, threading and rate limits. The policy is a dial to turn live during a demo. | An email tool for the agent |
| Human waits use **awakeables** behind a `HumanTasks` service | Uniform across VOs and workflows, a conversation can take several turns, and the browser never sees awakeable IDs | Workflow durable promises (resolve once) |
| Catalog of about 90 **fictional-brand** products on real standards, compiled into the binaries | No licensing or trademark issues, traps designed on purpose, every pair testable | Scraped PCPartPicker data (provenance), BuildCores OpenDB (non-commercial, gaps) |
| **Cancel** instead of delete | Invoices and audit need the record | Soft delete via `deleted_at` (same thing, less clear) |
| **No vector database.** Agents use structured catalog tools plus `list_*`/`read_*` tools over a dozen guide and policy documents | About 90 products and a dozen documents are too small for embeddings to add anything. Tool-based retrieval is deterministic and visible in the agent trace. | Vectorize with a chunking workflow, or AI Search (revisit only if the catalog grows a lot) |
| Services are generic over **ports** (SQL, LLM, classifier, mailer), with D1 or Workers AI adapters in production and a mock LLM in tests and offline dev (rusqlite comes post-plan) | Native unit tests now. `restate-e2e-harness` tests against a real `restate-server` post-plan. Cheap to keep from day one, expensive to retrofit. | Testing only through `wrangler dev` (slow, little Restate-level insight) |
| **restate-toolkit:** `restate-config`, `restate-ext`, `restate-e2e-harness` | Policies and identity as configuration, a replay-safe clock, terminal errors, durable polling, and e2e tests with journal and step-name checks | Hand-rolling each of these |

## Answers to the questions in the brief

- **Dataset and size.** There's no dataset that is free, safe to publish
  and complete enough for a correct checker. I recommend hand-curating
  **10–12 products per category across 8 categories**, with fictional
  brands on real standards.
  - More products don't make the demo better.
  - What matters is that every rule has a trap product and every price
    tier has alternatives.
  - Details are in [commerce.md §2](commerce.md#2-product-catalog).
- **Where investigations live.** They are their own entity, a workflow
  with its own admin list, linked to the ticket and the order. An admin
  can open one from an order without a ticket. A ticket shows its
  investigation inline.
- **What else the admin needs.**
  - A **human tasks inbox**, which is the centre of the admin side.
  - Payments ledger with the duplicate detector, refunds list, email log
    including rejected inbound mail, and the agent trace viewer.
  - Per-org stock editing (no price editing), settings (escalation
    threshold, AI reply policy and its threshold), and demo reset.
- **Email from an agent tool or from Restate.** From Restate, through
  the `Notifications` service. Agents return outcomes. Whether an
  AI-authored reply is sent automatically is the per-org setting
  described in [support.md §6](support.md#ai-reply-policy-org-setting).
  Reply confidence is the lower of the triage probability and a Clef
  "judge" pass over the draft.
- **Support as a workflow or a VO.** A VO per ticket, which is an event
  router with no LLM inside it. Long-running processes are workflows the
  ticket starts. Nothing ever waits while holding the ticket's lock.
- **The "Jev-type decision model".** I believe this is Cloudflare's
  **Clef** (`@cf/cloudflare/clef`, released 2026-10-01), which is
  reported to be Jev-compatible. It answers typed choice, yes/no and
  score questions with probabilities.

## What was missing from the brief (now in the plan)

1. **Demo reset and scenario presets.** Live demos need "an order
   shipped last Friday, lost" in one click, not five minutes of clicking.
2. **Short email-safe org IDs.** Clerk org IDs are long and mixed-case,
   so each org gets `support+k3x9q2ab@`.
3. **Sender verification details.**
   - DMARC result plus Clerk membership, with no reply to unknown
     senders (to avoid backscatter).
   - Auto-reply and loop protection.
   - Deterministic ticket IDs from `Message-ID`, so redelivery can't
     create duplicate tickets.
4. **A web channel for support:** "Send as email" in the app. Useful for
   presenting without an email client, and for local development.
5. **Explainability:** classification probabilities, the route taken,
   the agent trace (every LLM and tool call), order event timelines, and
   Restate UI deep links. The demo lives or dies on being able to *show*
   why something happened.
6. **Evals and calibration.** These set the confidence threshold from
   data, and they catch regressions when models or prompts change.
7. **Guardrails** after the agent: grounding in the catalog, and
   compatibility claims backed by an engine call.
8. **The AI reply policy** (always send, always review, or send above a
   confidence threshold) needs a *reply* confidence, not just a
   classification confidence. This plan adds a Clef judge for that.
9. **Real-time timers next to simulated time:** reminders, auto-close,
   and SLA nudges to admins.
10. **Cost and abuse controls.** Clerk decides who can sign up
    (opened for the demo period). The app adds per-org and global token
    budgets, rate limits, and org expiry.
11. **A minimal returns path** (shipped, then returned, then refunded),
    because "cancel a shipped order" needs somewhere to go.
12. **A "not a real shop" banner.** A realistic shop that sends real
    email can look like phishing.
13. **Email DNS:** Email Routing MX records on the apex and Resend on
    `send.` must coexist. Verify this in M0.
14. **Workers Paid plan.** The Free plan's 10 ms CPU and 3 MB bundle
    limits rule Rust and Restate out.

## Decisions log

| Date | Decision |
|---|---|
| 2026-10-13 | English UI, EUR prices (VAT-inclusive, single configurable rate). |
| 2026-10-13 | Fictional brands on real standards. |
| 2026-10-13 | Shipping policy in workdays after the ship date: ≤3 standard policy, 4 please wait, ≥5 investigation. |
| 2026-10-13 | One user is both customer and admin (the org creator). Invitations may come later. |
| 2026-10-13 | No `restate-worker` crate. Inline fetch glue, plus restate-toolkit (`restate-config`, `restate-ext`, `restate-e2e-harness`). |
| 2026-10-13 | No Vectorize. Retrieval through structured tools. |
| 2026-10-13 | AI reply policy is an org setting: `always_send`, `always_review`, `send_above_confidence`. |
| 2026-10-13 | Sign-up is controlled in Clerk: open during the demo period, restricted otherwise. There is no app-level sign-up gate. Token budgets and rate limits still apply. |
| 2026-10-13 | Tailwind CSS v4 + daisyUI 5 as the styling base, with Geist fonts and a light/dark switch in every header. |
| 2026-10-13 | UI components from dioxus-daisyui-components, except the theme switch (our own `ThemeToggle`, which persists the choice and avoids a flash). |
| 2026-10-13 | Local first: you configure the accounts after implementation ([setup.md](setup.md)), then the Go live milestone. The domain is already set up. |
| 2026-10-13 | The catalog and its content are generated without manual review, guarded by `xtask catalog validate`. |
| 2026-10-14 | The landing page is a public demo guide at `/demo`, linked from the permanent banner. Catalog browsing stays public too (compiled into the SPA); everything that belongs to a shop needs sign-in. |
| 2026-10-13 | Work goes straight to `main`. |
| 2026-10-13 | End-to-end tests (restate-e2e-harness, rusqlite backend) are deferred until after the plan. The port seams stay from day one. |
| 2026-10-13 | An `LLM_BACKEND=mock` mode lets the stack run without a Cloudflare account. |
| 2026-10-14 | allama replaces anyllm for LLM calls: chat with tools and Clef decisions through one provider-agnostic library (`allama-cloudflare-worker` in Workers, `allama-openai-compat` + `allama-systemone` over HTTP for evals). |
| 2026-10-14 | Email is our own `Email` service in the services worker over a `Mailer` port (Resend in production, Mailpit's HTTP API locally), not `restate-email` and not a separate `restate-email-endpoint` container. Member lookups (`MemberDirectory`) belong to mail-in only. Demo flows are shell scripts (`scripts/smoke.sh`, `scripts/scenarios.sh`), not xtask commands. |
| 2026-10-14 | CI is a Dagger v1 workspace: the daggerverse `rust` module plus a local Dang module `ci` (deny, catalog, Worker and SPA release builds, deploy). The services worker deploys as a version whose preview URL is registered with Restate before promotion. |
| 2026-10-13 | `worker`/`worker-macros` are pinned to `=0.8.5` and `wasm-bindgen` to `=0.2.126`: newer `worker` needs wasm-bindgen ≥ 0.2.128, but dx 0.7.10 embeds 0.2.126. Dependabot ignores the three. |
| 2026-10-13 | `AUTH_MODE=dev`: a dev sign-in (unsigned `dev.` tokens naming an org and a user) so the whole stack runs without a Clerk account; mail-in checks members against D1 `org_members` then. Never in production, and the services worker refuses to start without Restate identity keys unless it is set. |
| 2026-10-14 | A reship is a store-paid replacement order (`payment_method = store`, no payment, free shipping, `replacement_for` the original) placed through `Order/place`, so it ships and is tracked like any other order. |
| 2026-10-14 | CI moves from Dagger to GitHub Actions (Dagger runs took too long): one job per `justfile` recipe in the devenv shell, as in sagikazarmark/octoevents. Deployment is `scripts/deploy.sh` (manual Deploy workflow, same steps as before: migrations, services version registered with Restate, then promoted, then web and mail-in). `dagger.toml` and `ci/` stay in the repository, unused. |
| 2026-10-14 | Two deployments of one build: `dev` (`techdepo-dev-*` Workers, shop on `techdepo-dev-web.vibekoders.workers.dev`, Clerk development instance, `support-dev+<org>@techdepo.dev`) and `live` (`techdepo-live-*`, `techdepo.dev`, Clerk production instance, `support+<org>@`), each with its own D1 and Restate Cloud environment. `[env.dev]`/`[env.live]` in the wrangler.toml files, GitHub environments `dev`/`live`, one manual Deploy workflow with an environment input ([../deploy.md](../deploy.md)). |
| 2026-10-14 | The Workers use only Workers AI through the AI binding; the eval models come from `allama-providers` model addresses (Anthropic, OpenAI, Azure, Gemini, Cloudflare), xtask features, local tooling only. |
| 2026-10-14 | Models are named by address, `<provider>:<model>` as in pydantic-ai (`azure:<deployment>`, `azure-responses:<deployment>`; a bare ID is Workers AI): the default models (`AI_AGENT_MODEL`, `AI_CLASSIFIER_MODEL`) and the eval flags take them, logs and the Muhaha page show them. The stored model settings stay `{provider, model}`. Azure OpenAI (Chat Completions and Responses) joins the Workers' providers, needing `AZURE_OPENAI_API_KEY` and `AZURE_OPENAI_RESOURCE`. |

## Research notes

These are the facts the plan relies on, checked on 2026-10-13. Re-verify
anything time-sensitive.

- **Dioxus:**
  - `dioxus` 0.7.10 is current.
  - Fullstack on Workers is open upstream (dioxus#3275, #3601), and
    `dioxus-cloudflare` is yanked.
  - `dioxus-clerk` 0.6 has a `worker` feature for `ClerkAuthLayer`.
  - `dioform` is at 0.9.
  - `dioxus-daisyui-components` is a `dx components` registry, not on
    crates.io.
- **workers-rs 0.8.7:**
  - Supports D1, Workers AI, Static Assets, Queues, axum and
    `#[event(email)]` with `reply`/`forward`.
  - Has **no Vectorize binding** (#709). This doesn't matter now that
    Vectorize is dropped.
- **Restate:**
  - `restate-sdk` 0.12.1 is current, with no official wasm support.
  - The patch is upstream PR #128 / the `wasm` branch of
    sagikazarmark/restate-sdk-rust.
  - Workers runs in request-response mode only.
  - Ingress supports `Idempotency-Key` with 24 h dedupe by default (we
    configure 7 days, architecture §2), and awakeables
    can be resolved over HTTP.
  - restate-toolkit 0.3 (`restate-config`, `restate-ext`,
    `restate-e2e-harness`) targets sdk 0.12.
    - `restate-ext` works on wasm without the `shutdown` feature.
    - `restate-e2e-harness` is native and Unix-only.
- **email-rs 0.10:**
  - Provides the Resend, Cloudflare `send_email`, lettre SMTP and Restate
    transports.
  - `restate-email` provides a durable `Email/send` service, but it can't
    be bound on Workers (it enables restate-sdk's default features, and
    its transport futures are `!Send` on wasm). We use `email-message`
    and our own `Email` service instead (architecture §7).
  - It has no templating.
- **allama** (replaced anyllm on 2026-10-14):
  - Provider-agnostic chat, tools, structured output, embeddings and
    decision models, building for wasm32.
  - `allama-cloudflare-worker` runs Workers AI chat with tools *and* Clef
    decisions through the AI binding. It works with any `worker` 0.8, so it
    fits our 0.8.5 pin.
  - No agent loop by design; `crates/agent` runs the tools.
- **Workers AI:**
  - Clef and `clef-flash` handle classification with per-option
    probabilities.
  - Function-calling models include `llama-3.3-70b-instruct-fp8-fast`,
    `gpt-oss-120b`, `qwen3-*`, `llama-4-scout` and others.
- **Email Routing:**
  - Subaddressing keeps `+tag` in `message.to`.
  - `reply()` only works under strict conditions, so replies go through
    Resend instead.
- **Datasets:**
  - docyx/pc-part-dataset is a PCPartPicker scrape, last updated
    2025-07.
  - BuildCores OpenDB is CC BY-NC-SA, and its schemas are a useful
    reference.
  - iBlessi/pc-builder-parts-2026 is CC BY 4.0, with 126 parts and no
    coolers.
