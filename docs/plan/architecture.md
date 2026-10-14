# Architecture

## 1. Deployables

All three workers run on Cloudflare Workers, and Restate runs on
Restate Cloud.

```
                          ┌───────────────────────────── Cloudflare ──────────────────────────────┐
 Browser ── HTTPS ──────► │ techdepo-web       Dioxus SPA (Static Assets) + /api/* (axum on      │
  (Clerk session JWT)     │                    workers-rs). Reads D1. Mutations → Restate ingress │
                          │                                                                       │
 Customer mail ─ MX ────► │ Email Routing ──► techdepo-mail-in   Email Worker: parse, verify,    │
                          │                                       dedupe → Restate ingress /send  │
                          │                                                                       │
 Restate Cloud ─ HTTPS ─► │ techdepo-services  Restate endpoint (request-response mode): all     │
  (signed requests)       │                    services, VOs, workflows, agents. D1, Workers AI,  │
                          │                    Resend (our `Email` service)                       │
                          │                                                                       │
                          │ D1 (techdepo)          KV (caches)          Workers AI                │
                          └───────────────────────────────────────────────────────────────────────┘
           Restate Cloud ◄── ingress (Bearer API key, Idempotency-Key) ── web, mail-in
           Resend ◄── HTTPS ── techdepo-services (`Email` service, `Mailer` port)
           Clerk  ◄── JWKS / Backend API ── web, mail-in
```

- **techdepo-web** serves the UI and the API. It never runs long work.
  - Reads go straight to D1.
  - Every state change goes through a Restate handler, called
    request-response through ingress. This is a deliberate rule: all
    mutations are journaled, retried and visible in the Restate UI.
- **techdepo-mail-in** is a thin adapter between the edge and Restate. It
  contains no AI and no business logic beyond identification and
  validation (see [support.md](support.md#1-intake)).
- **techdepo-services** is one Restate deployment hosting every service.
  - Splitting it later (commerce, support, agents) is just a matter of
    registering another deployment, because callers don't change.
  - Keep it as one worker until the bundle size or CPU limits push back.
- **One D1 database** is shared by all workers through bindings.
  - D1 holds the **read model and records**: orders, invoices, tickets,
    traces.
  - Restate state holds **process state**, e.g. "which awakeable is this
    ticket waiting on".

### Why not Dioxus fullstack (server functions, SSR) on Workers

Dioxus 0.7 fullstack pulls in Tokio networking that doesn't compile for
Workers, and the bridge crate (`dioxus-cloudflare`) is yanked. The proven
pattern, also used by the `dioxus-clerk` demo, is a **CSR SPA served as
Workers Static Assets plus explicit `/api/*` routes in an axum router on
workers-rs**. Request and response types live in a shared crate, so the
front end still gets a typed client. Revisit this if Dioxus gains a
Workers target (dioxus#3275).

Files are served straight from Static Assets. A path that matches no
file reaches the worker, which serves `index.html` only for extension-less
paths (client-side routes); a missing file such as `/assets/old.js` is a
`404`, not the shell with a `200` (wrangler's built-in
`single-page-application` mode would do the latter).

## 2. Restate on Workers

- **SDK version:** `restate-sdk` 0.12 from the
  [`wasm` branch of sagikazarmark/restate-sdk-rust](https://github.com/sagikazarmark/restate-sdk-rust/tree/wasm),
  which matches upstream PR #128. It is applied with a workspace patch.
  Because `restate-toolkit` depends on `restate-sdk = "0.12"`, the same
  patch covers it too:

  ```toml
  [patch.crates-io]
  restate-sdk = { git = "https://github.com/sagikazarmark/restate-sdk-rust", branch = "wasm" }
  ```

  - `Cargo.lock` pins the commit.
  - `deny.toml` has `unknown-git = "deny"`, so the fork is listed in
    `[sources] allow-git` (next to `DioxusLabs/components`).
- **Use `default-features = false`.** Enable `rand`, `uuid` and
  `rust_crypto`; `http_server` pulls in tokio/net.
  - getrandom needs the `js` backend (0.2) and the `wasm_js` backend
    (0.3/0.4), plus the cfg flag if it is still required.
- **No adapter crate.** The services worker's `#[event(fetch)]` does
  three things:
  - converts `worker::Request` into an `http::Request`
  - calls `endpoint.handle_with_options(req, HandleOptions { protocol_mode: RequestResponse })`
  - converts the response back

  That is a few dozen lines in `apps/services/src/lib.rs`.
- **Request-response mode only.** Workers buffer the request body.
  Locally, register with `--use-http1.1`.
- **Send bounds:** wrap JS-backed futures (D1, AI, Fetch) in
  `worker::send::SendFuture`.
  - This happens only inside the wasm adapter implementations (see §3),
    so service code never sees it.
- **Limits:**
  - **The Workers Paid plan is required.** The Free plan allows 10 ms CPU
    per request, which a handler attempt (Restate journal replay, D1, JSON)
    does not fit. The bundle itself would fit the Free plan's 3 MB (see
    below).
  - Raise `limits.cpu_ms` for the services worker.
  - LLM calls are I/O wait, so they don't consume CPU.
  - Inactivity and abort timeouts are set per service through
    `restate-config` (below), e.g. 5 min for the agent handlers.
- **Measured size and startup** (2026-10-14, everything linked: the SDK
  fork, restate-config/ext, allama with the Workers AI and mock backends,
  `email-message` (our `Email` service; `restate-email` itself can't be
  bound on Workers), askama templates, the compiled-in catalog and compat
  engine, D1 adapter; 20 services, 58 handlers in one endpoint). Release
  profile (`opt-level = "z"`, fat LTO, one codegen unit, `panic = "abort"`)
  plus worker-build 0.8.5's `wasm-opt` (binaryen 132):

  | Build | `index_bg.wasm` | gzip |
  |---|---|---|
  | `worker-build --release` | 6.14 MiB (6,435,172 B) | 1.85 MiB (`gzip -9`) |
  | `wrangler deploy --dry-run` total upload | 6,332 KiB | **1,921 KiB** of the 10 MB limit |
  | `worker-build --dev` (local only) | 30.2 MiB | 4.65 MiB |

  - **Startup** (`wrangler check startup`, local workerd, 3 runs): 15-18
    ms profiled (wasm instantiation and the wasm-bindgen glue), against
    Cloudflare's 1 s startup limit. V8 compiles wasm lazily: decoding the
    module takes about 10 ms, an eager baseline (Liftoff) compile of the whole module 46 ms,
    an eager optimizing (TurboFan) compile 0.7 s (node 22, same V8 family);
    workerd never does the latter up front.
  - **First request** on a fresh `wrangler dev` (release build), `GET
    /discover`: 15-24 ms, then 2-3 ms warm (the endpoint and its service
    policies are rebuilt per request, that is in the warm figure).
  - The release build of the worker crate and its workspace dependencies
    takes about 1m16s with 1.5 GB peak RSS on the dev machine (cargo capped
    at 2 jobs).
  - **Verdict:** no pressure to split the services worker or trim crates;
    re-measure when a big dependency lands. To reproduce: `(cd
    apps/services && wrangler deploy --dry-run --outdir /tmp/svc && wrangler
    check startup)`.

### restate-toolkit

| Crate | Use |
|---|---|
| `restate-config` | Endpoint and service policy from one JSON config, passed in as a wrangler var or secret. It holds `identity_keys` (Restate Cloud request identity, with rotation) and per-service and per-handler policies. **Policies used:** <ul><li>`ingress_private = true` for every internal service (`Router`, agents, `Notifications`, `Email`, `Payments`, `Carrier`), so only the entry handlers are reachable from ingress.</li><li>Retry policy with `on_max_attempts = "pause"` for money-moving steps: a stuck refund pauses for a person instead of being killed.</li><li>Agents keep the default timeouts after all: in request-response mode Restate ignores inactivity timeouts (RT0021), and the default 10-minute abort timeout is plenty for a few model calls.</li><li>Journal, idempotency and workflow retention of 7 days on every service (`policy::with_retention`, filled in under any policy or override that sets none): completed journals stay inspectable from the admin pages' Restate UI links, an `Idempotency-Key` (client key, inbound `Message-ID`) dedupes past a sending MTA's retry window, and a refund or investigation ID cannot run twice. Idempotency and workflow retention also cap the journal retention of those invocations, so they are never shorter.</li></ul> |
| `restate-ext` | <ul><li>**Replay-safe clock** (`ctx.now()`). Every timestamp: order events, invoice dates, ship dates, and the base of each order's virtual clock.</li><li>**`TerminalErrorExt`** for invalid state transitions, e.g. "cannot ship an unpaid order" becomes a terminal 409 instead of being retried.</li><li>**Durable polling** (`Poller`) for the carrier trace: poll `track()` every N seconds with a timeout, as a real carrier integration would.</li></ul> |
| `restate-e2e-harness` (**post-plan**, see [roadmap.md](roadmap.md#post-plan-hardening)) | End-to-end tests of every service against a real `restate-server` (native, Unix): <ul><li>deploys the endpoint in-process and invokes it over ingress</li><li>inspects journals and `ctx.run` names, and watches retries</li><li>kills or cancels invocations</li></ul> The **step-name table** (the ordered `ctx.run` names per handler path) is checked in CI, so changes that would break in-flight journals on redeploy are caught in review. |

**Consequence:** service code must also compile **natively**, so
`crates/services` depends only on traits (ports).
- **Now:** production binds the ports to Workers adapters. The agent and
  route unit tests use a mock LLM. CI runs `cargo check` on
  `crates/services` for the native target, so wasm-only code can't leak
  in.
- **Post-plan:** the rusqlite backend and the harness suites. The seams
  exist from day one, because retrofitting them later is expensive.

## 3. Cargo workspace layout

```
crates/
  domain/          Pure types and rules: ids, Money, order state machine,
                   workday calendar, shipping policy, invoice numbering format.
  catalog/         Typed product specs; catalog + guides/policies embedded at compile
                   time (include_str! of data/) – single source of truth.
  compat/          Compatibility rules engine (pure; compiled into SPA + workers).
  api-types/       DTOs for /api/* shared by SPA and web worker; typed client.
  ports/           Traits the services depend on: SqlExecutor, Llm, Classifier, Mailer
                   (only our `Email` service calls it). MemberDirectory (Clerk, or D1
                   `org_members` in dev) is used by mail-in's sender check only. Time comes
                   from restate-ext's journaled clock, so it needs no port.
  db/              Repositories (SQL + row mapping) over `SqlExecutor`.
                   Backend: D1 (wasm, SendFuture-wrapped). Post-plan: rusqlite (native,
                   e2e) with the same SQL and migrations – D1 is SQLite.
  llm/             `Llm`/`Classifier` impls over allama: allama-cloudflare-worker
                   (chat with tools + Clef decisions through the AI binding, wasm);
                   `mock` – deterministic, keyword-based (LLM_BACKEND=mock: local dev
                   without a Cloudflare account, unit tests); Anthropic, OpenAI,
                   Azure OpenAI, Gemini and OpenRouter over the Workers Fetch API
                   when their secrets are set; `ModelRegistry`, the `Models` port behind the operator's
                   per-task model settings. Native `xtask eval` builds its providers
                   with allama-providers.
  agent/           Minimal durable agent loop + Tool trait (see support.md §5).
  services/        ALL Restate services/VOs/workflows/agents, generic over ports.
                   Builds for wasm32 and native.
apps/
  web/             Dioxus 0.7 SPA (tailwind v4 + daisyUI 5 + dioxus-daisyui-components,
                   dioform, dioxus-clerk).
  web-worker/      techdepo-web: axum /api/*, ClerkAuthLayer (dioxus-clerk `worker`
                   feature), Static Assets for the SPA build output.
  mail-in/         techdepo-mail-in: #[event(email)] (workers-rs ≥ 0.8.4).
  services/        techdepo-services: thin wasm entry – builds the Endpoint from
                   crates/services with Workers adapters, restate-config, fetch glue.
tests/e2e/         (post-plan) restate-e2e-harness suites: services + rusqlite + mock
                   LLM + a recording `Mailer` capturing sent mail.
xtask/             seed, catalog validate, dev-vars, evals. Demo flows are shell scripts
                   (scripts/smoke.sh, scripts/scenarios.sh).
data/
  catalog/         *.yaml specs + per-product Markdown content.
  kb/              Policies (shipping, returns, refunds, warranty) and buying guides
                   as short Markdown documents with front-matter (slug, title, summary).
migrations/        D1 SQL migrations (wrangler d1 migrations).
evals/             Support email fixtures + expected outcomes.
deploy/            compose.yaml (local infra: Restate + Mailpit).
scripts/           local stack (dev.sh, dev-down.sh), SPA build, smoke/scenario runs, deploy.sh.
justfile           the CI recipes; .github/workflows runs one job per recipe (§9).
docs/plan/         This plan.
```

**Notes:**

- **No `products` table.** The catalog is global and immutable, so it is
  compiled into the binaries and served to the SPA as
  `GET /api/catalog`, cacheable by catalog version.
  - Order lines snapshot the name, SKU and price.
  - Only **per-org stock** lives in D1.
- **No vector database.** About 90 products and a dozen policy and guide
  documents are small enough for **structured tools**:
  - typed filters over the in-memory catalog
  - `list_guides`, then `read_guide(slug)`

  The model chooses what to read, so retrieval becomes part of the agent
  demo rather than a hidden pipeline. Revisit Vectorize or AI Search only
  if the catalog grows by an order of magnitude.
- **`dioxus-daisyui-components`** is a `dx components` registry rather
  than a crate, so its components are copied into `apps/web/src/components/`.
  - Add `@source` lines for any classes coming from dependency crates.
- **Tailwind:** a `tailwind.css` at the crate root containing
  `@import "tailwindcss"; @plugin "daisyui";`. `dx` runs the Tailwind
  watcher itself.
  - Use daisyUI themes for the modern look, e.g. a custom
    "techdepo" light/dark pair, with a theme controller in the navbar.

## 4. Data model (D1)

Every table carries `org_id`, and every query filters on it. The `db`
crate's API takes an `OrgScope` value that can only be built from a
verified session or a verified inbound email. The one exception is
`app_settings`: the operator's app-wide settings (the model settings),
written by the `Muhaha` service only.

| Table | Purpose |
|---|---|
| `orgs` | clerk_org_id, public_id (email tag), settings JSON (escalation threshold, AI reply policy and threshold, fault defaults, durations, holidays), created_at, expires_at (demo org expiry), features JSON (`domain::OrgFeature`: parts of the app only some orgs get, switched on by the operator, not by admins) |
| `inventory` | (org_id, product_id) → stock |
| `carts`, `builds` | per user |
| `orders`, `order_items` | read model written by the `Order` VO; `virtual_clock_offset`, `cancelled_at`, `replacement_for` |
| `order_events` | append-only timeline (state changes, emails, invoices, tickets, faults) |
| `payments` | charges and refunds (fake PSP ledger) |
| `invoices` | kind invoice/credit_note, status issued/voided, `supersedes_id`, billing snapshot, lines, per-org sequence |
| `invoice_sequences` | per org per year, for atomic `UPDATE … RETURNING` |
| `shipments`, `carrier_events` | fake carrier: tracking, ETA, hidden lost outcome |
| `tickets`, `ticket_messages` | conversation, classification and probabilities, route, assignee (ai/human), status |
| `investigations`, `refunds` | records mirrored from their workflows |
| `human_tasks` | kind, payload, allowed decisions, awakeable_id, status, decided_by, decision, SLA |
| `email_log` | inbound (incl. rejected + reason) and outbound copies, Message-IDs for threading |
| `agent_runs`, `agent_steps` | traces: model, prompts, tool calls, results, tokens, latency, reply confidence |
| `llm_usage` | per-org daily token accounting (budget enforcement) |
| `app_settings` | app-wide, no `org_id`: `models` (`domain::ModelSettings`), the operator's choice of model per AI task |

## 5. Authentication and authorization

- **Front end:** `dioxus-clerk`, with `ClerkProvider`, `SignIn`,
  `OrganizationSwitcher`, `UserButton`, and `Protect` for admin routes.
- **Clerk settings:**
  - "Membership required" is on, so every user works inside an org.
  - After sign-up, the user creates their shop org, or one is created
    for them, and becomes `org:admin`.
  - **That one user is both the customer and the admin.** Invitations are
    out of scope for now. The data model already separates `user_id` on
    orders from the org, so adding invitations later is just a Clerk
    setting.
  - **Sign-up is controlled entirely in Clerk** (open during the demo
    period, restricted mode otherwise). The app has no allowlist or
    sign-up gate of its own.
- **Web worker:**
  - `ClerkAuthLayer` (`worker` feature) verifies the session JWT against
    JWKS, using `jsonwebtoken` 11 with the rust-crypto backend.
  - The org ID and role come from the token claims.
  - Admin endpoints require `org:admin`.
- **Services worker:**
  - Trusts its callers through Restate request identity (`restate-config`
    `identity_keys`).
  - Handlers receive `org_id` in their input from the web worker, which
    already authenticated the user.
  - Ingress API keys stay server-side and are never sent to the browser.
- **Mail-in:** verifies the sender against Clerk (see
  [support.md](support.md#1-intake)).

## 6. LLM and AI services

| Use | Choice |
|---|---|
| Ticket classification, reply interpretation, reply confidence | **Workers AI `@cf/cloudflare/clef`** (or `clef-flash`). It answers typed `choice`/`noul`/`score` questions with a probability for every option in one pass, which gives a real confidence value for escalation and for the AI reply policy. Called through allama's `DecisionProvider` (allama-cloudflare-worker runs Clef through the AI binding; allama-systemone over HTTP), which models typed questions and per-option probabilities. Clef is reported as Jev-compatible, which I take to be the "Jev-type decision model" in the brief. |
| Agents (tool calling) | Workers AI function-calling model through allama's `ChatProvider` (`allama-cloudflare-worker`). Configurable; start with `@cf/meta/llama-3.3-70b-instruct-fp8-fast` and compare `gpt-oss-120b` and `qwen3` with the evals. Workers AI takes no `tool_choice`, and the agent loop is designed around that. |
| Fallback classification | JSON-schema structured output (allama `extract`) with a self-reported confidence, using the agent model. This is weaker and only used when Clef cannot answer a request (an answer that does not match the questions): throttling, 5xx and network errors are retried against Clef by the caller (Restate step retries), and a Clef that stays down ends in an escalation, never in uncalibrated confidence. `AI_FALLBACK_CLASSIFIER=off` disables it. |
| Offline (`LLM_BACKEND=mock`) | `crates/llm` `mock`: a keyword classifier with calibrated-looking probabilities (vague, angry, multi-intent and injected messages come out uncertain) and a scripted agent model that calls the real tools and answers from their results. Every demo scenario runs without a Cloudflare account. |
| Retrieval | No embeddings: catalog search and guide reading are tools (see [support.md §7](support.md#7-knowledge-without-a-vector-database)). |
| Choosing models (operator) | The rows above are the deployment's *defaults* (`LLM_BACKEND`, and `AI_*` as model addresses `<provider>:<model>`, a bare ID being Workers AI; `domain::ModelChoice::parse`). The operator overrides them per task on the Muhaha page (`domain::ModelSettings` in `app_settings`): product advisor, order assistant, triage, offer replies, judge, and the optional classifier fallback, each a provider plus a model ID. Providers: mock, Workers AI (the `AI` binding), and Anthropic, OpenAI, Azure OpenAI (`azure` Chat Completions and `azure-responses`, both on `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_RESOURCE`), Gemini and OpenRouter, each available once its secrets are set on the services worker (`ANTHROPIC_API_KEY`, …); their requests go through the Workers Fetch API (`techdepo_llm::WorkerFetch`, an allama `HttpTransport`), not reqwest. A classifier on a decision model (Clef) is calibrated; on any other model it is JSON mode with self-reported confidence. `ports::Models` (`techdepo_llm::ModelRegistry`) turns a choice into an adapter; a provider without credentials fails every call for good (the caller escalates). Every consumer reads the settings **once per invocation, in a journaled `ctx.run` (`model-settings`)**: retries and replays see the same models, and the `Investigation` workflow reads them at its start, so changing models never changes a running workflow. |

## 7. Outgoing email

- **Our own `Email/send` service** (`crates/services`), built on
  email-rs's `email-message` types. `restate-email` itself can't be bound
  on Workers: it enables restate-sdk's default features (tokio net), and
  its transport futures are `!Send` on wasm while `ctx.run` needs `Send`.
  Every outbound email goes through our `Notifications` service, which
  renders a template and calls `Email/send` (see
  [support.md](support.md#6-outbound-email)). `Email/send` is the only
  caller of the `Mailer` port and the only writer of outbound `email_log`
  rows.
- **Templates:** `askama`, compile-time and wasm-safe, producing HTML
  plus a text alternative. email-rs has no templating of its own.
- **Transports** (`apps/services/src/mailer.rs`, picked by
  `EMAIL_TRANSPORT`), both plain HTTP through the Workers Fetch API:
  - **Production:** **Resend** (`POST https://api.resend.com/emails`, the
    `Message-ID` as `Idempotency-Key`, so a retried send is delivered
    once).
  - **Local:** **Mailpit**'s HTTP send API (`POST
    {MAILPIT_URL}/api/v1/send`, default `http://localhost:8025`).
  - The `Email` service is bound in the services worker in both cases;
    only the `Mailer` adapter changes.
- **Tests:** `testing::RecordingMailer` (`techdepo_ports`) records every
  send; the post-plan e2e suites use it too.
- **DNS checklist:**
  - Cloudflare Email Routing MX records on the apex `techdepo.dev`.
  - Resend DKIM (`resend._domainkey`) and its return-path MX on
    `send.techdepo.dev`.
  - One SPF record that includes both.
  - DMARC on the apex.
  - Verify all of this early, because it is the most common silent
    failure.

## 8. Local development

| Concern | Local | Deployed (`dev`, `live`) |
|---|---|---|
| Workers | one `wrangler dev` per worker on fixed ports: web 8787, services 8788, mail-in 8789. All use the same `--persist-to .wrangler/state` so they share local D1. (Multi-config mode only exposes the first worker, and Restate has to reach the services worker directly.) | `wrangler deploy` per worker |
| SPA | `dx serve` with `[[web.proxy]]` → `http://localhost:8787/api/` for hot reload | built into the Static Assets of techdepo-web |
| D1 | miniflare's local SQLite, created by `wrangler d1 migrations apply --local` | D1 |
| Restate | `restate-server` in docker compose (8080 ingress, 9070 admin/UI); `restate deployments register --use-http1.1 http://host.docker.internal:8788` | Restate Cloud; register the Workers preview URL from CI |
| Outgoing email | Mailpit's HTTP send API (`EMAIL_TRANSPORT=mailpit`, `POST :8025/api/v1/send`; UI 8025), called by the `Email` service in the services worker | Resend |
| Incoming email | `curl -X POST 'localhost:8789/cdn-cgi/handler/email?from=…&to=…' --data-binary @mail.eml` (wrangler's local email trigger), mail-in's dev-only `POST :8789/inbound?from=…&to=…` (answers with the outcome), the in-app "Send as email" form, or `scripts/scenarios.sh` | Cloudflare Email Routing |
| Workers AI | `LLM_BACKEND=mock` by default (no binding, no account). `LLM_BACKEND=workers-ai` in `.env` makes `scripts/dev.sh`/devenv run the services worker with `--env workers-ai`: a remote AI binding (`remote = true`) that needs `CLOUDFLARE_API_TOKEN` + `CLOUDFLARE_ACCOUNT_ID`. It is not top-level because wrangler refuses to start any worker declaring an AI binding without a token. | binding |
| Auth | `AUTH_MODE=dev`: a dev sign-in (`POST /api/dev/login`, unsigned `dev.` tokens, no account); or a Clerk development instance with `AUTH_MODE=clerk` | `AUTH_MODE=clerk`: a Clerk development instance on dev, the production instance on live |

- **Orchestration:**
  - `deploy/compose.yaml` runs Restate and Mailpit.
  - `devenv up` (`devenv.nix`) starts everything in the foreground:
    `processes` for the compose infra, the three `wrangler dev` workers and
    `dx serve`, and one-shot `tasks` around them: `techdepo:dev-vars`,
    `techdepo:migrate` (local D1), `techdepo:build-web` (only when
    `apps/web-worker/public` has no build) and `techdepo:register`, which
    runs whenever the services process becomes ready.
  - Builds never overlap (the dev machine has ~8 GB and no swap): each
    step waits for the previous one to answer on its port, in the order
    infra → services (+ register) → SPA build → web → mail-in → dx.
    `devenv up web` starts a subset with what it needs.
  - `scripts/dev.sh` starts the same stack in the background (logs in
    `.wrangler/dev/logs`, `--no-dx`, `--restart <part>`);
    `scripts/dev-down.sh` stops it.
- **Seeding:** `xtask seed --local` applies migrations.
- **Configuration:**
  - The root `.env` (git-ignored; template in `.env.example`) is loaded
    by devenv.
  - `xtask dev-vars` writes each worker's `.dev.vars` (git-ignored)
    with only the keys that worker needs.
  - The SPA gets the Clerk publishable key at runtime from
    `GET /api/config`, so one build works in every environment.
- **Without accounts:**
  - `LLM_BACKEND=mock` runs the support pipeline offline, with
    deterministic classification and canned agent replies.
  - `AUTH_MODE=dev` replaces Clerk with a dev sign-in (any name and
    email, see [api.md](api.md)); never in production.
  - `EMAIL_TRANSPORT=mailpit` sends every email to Mailpit.

## 9. CI and deployment

- **GitHub Actions** (`.github/workflows/`), modelled on
  sagikazarmark/octoevents: every step runs in the devenv shell
  (`devenv shell -- bash -e {0}`, set up by `sagikazarmark/setup-devenv`),
  so CI uses exactly the local toolchain, and every job is one recipe of
  the root `justfile` (`just ci` runs them all locally, heavy builds
  last). Actions are pinned by commit SHA; Dependabot (`.github/dependabot.yaml`)
  updates cargo (except the `worker`/`worker-macros`/`wasm-bindgen` pins),
  npm (`apps/web`) and the actions daily.
- **`ci.yaml`** (push to `main`, pull requests), one job per recipe:
  - `fmt`: `cargo fmt --check`.
  - `check`: the wasm32 workspace (everything that ships, without
    `xtask`) and the native workspace with all targets (without the
    wasm-only `techdepo-services-worker` and `techdepo-web`).
  - `clippy`: the same two sets, `--deny warnings`.
  - `test`: every native test, including `crates/services` and
    `crates/db` on SQLite, mail-in's intake, the web worker's route
    table and the exhaustive compatibility snapshot test (`crates/compat`,
    insta).
  - `deny`: `cargo deny check` (`deny.toml`).
  - `catalog`: `xtask catalog validate`.
  - `eval`: the classification and agent evals on the mock models
    (deterministic; they fail below the eval files' minimums).
  - `build-workers`: release builds of the three workers (worker-build).
  - `build-web`: the SPA release build (`scripts/build-web.sh`).
  - post-plan: the `restate-e2e-harness` suites and step-name tables.
- **`analysis-scorecard.yaml`**: OpenSSF Scorecard on `main` and weekly.
- **Two deployments** of the same build ([../deploy.md](../deploy.md)):
  `dev` (`techdepo-dev-*` Workers, the shop on
  `techdepo-dev-web.vibekoders.workers.dev`) and `live` (`techdepo-live-*`,
  the shop on `techdepo.dev`), each with its own D1 database, Restate Cloud
  environment, Clerk instance and support local part (`support-dev`,
  `support`). Their committed values are `[env.dev]`/`[env.live]` in the
  wrangler.toml files.
- **Deploy** (`.github/workflows/deploy.yaml`, manual `workflow_dispatch`
  with an `environment` input, GitHub environment `dev` or `live`, live only
  from `main`, only when the repository variable `DEPLOY` is `true`) runs
  `scripts/deploy.sh --env <dev|live>`, which also works from a shell with
  the values in `deploy/<env>.env`:
  - Release builds of the SPA and the three workers, one at a time.
  - `wrangler d1 migrations apply DB --remote` on the environment's database.
  - The services worker is uploaded as a new version (`wrangler versions
    upload`), its preview URL is registered with the environment's Restate
    Cloud (`RESTATE_ADMIN_URL`, `RESTATE_ADMIN_TOKEN`, HTTP/1.1), and only
    then is the version promoted to 100%. Every version keeps its own
    immutable URL, so in-flight invocations finish on the code that started
    them, and the two environments never share one.
  - Then the web and mail-in workers are deployed, with the
    environment-specific values as `--var`s (`RESTATE_INGRESS_URL`, and the
    Clerk publishable key and authorized parties for the web worker). The
    optional model variables (`LLM_BACKEND`, `AI_*`, token budgets) are
    passed the same way when the environment sets them.
  - The deployed `wrangler.toml` (`apps/*/wrangler.deploy.toml`, removed
    afterwards) drops the `[build]` table (the build is done) and gets
    `D1_DATABASE_ID` in place of the committed placeholder.
  - `--secrets` also uploads each worker's secrets from the environment
    (the workflow's `secrets` input); `--dry-run` builds and checks the
    three workers without uploading anything.
- **Evals on real models** run on demand, not on every push, because they
  cost money.

## 10. Risks

| Risk | Mitigation |
|---|---|
| Restate SDK wasm patch is unmerged upstream | The lock file pins the fork's commit. CI builds for both wasm and native. Track PR #128. |
| Services must build for both wasm (production) and native (unit tests now, e2e post-plan) | Ports and adapters from the first commit. CI builds both targets, and the wasm-only code stays in `apps/services` and the adapters. |
| (post-plan) D1 and rusqlite drift (D1 SQL quirks, limits) | Shared migrations. Keep the SQL to the SQLite subset that D1 supports. A small smoke suite also runs against `wrangler dev`. |
| Worker bundle size or startup time (Rust + many crates) | Paid plan (10 MB compressed limit). `opt-level="z"`, LTO, `wasm-opt`. Measured with everything linked ([§2](#2-restate-on-workers)): 1.9 MiB compressed, ~15 ms startup. Re-measure when a big dependency lands. |
| `!Send` JS futures throughout | Wrap them once in the wasm adapters (`SendFuture`). |
| Clef is two weeks old | `Classifier` trait with a JSON-mode fallback implementation. |
| Workers AI tool-calling quality | Evals per model. A small, sharp tool set. Guardrails plus escalation. |
| Public demo cost and abuse | Clerk controls who can sign up. Per-org and global LLM budgets and email rate limits apply (see [support.md](support.md#9-safety-cost-abuse)). The web API limits every mutation per user (60 per minute) and orders per org (`MAX_ORDERS_PER_HOUR`), see [api.md](api.md#rate-limits). |
