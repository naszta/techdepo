# TechDepó

Demo PC-parts webshop showing **what should be code, a durable workflow, an AI agent, or a human decision**.
Rust everywhere, deployed on Cloudflare Workers, durable execution with Restate.

The plan lives in `docs/plan/` (README, architecture, commerce, support, design, setup, roadmap). It is the spec:
read the relevant section before implementing a feature, and keep it in sync when a decision changes.

## Layout

```
crates/domain      pure types/rules: ids, Money (i64 cents, EUR), order state machine, workday calendar,
                   shipping policy, invoice numbering, org settings
crates/catalog     typed product specs + the catalog compiled in from data/catalog (include_str!)
crates/compat      compatibility rules engine (pure; used by SPA, web worker, agents)
crates/api-types   DTOs for /api/* shared by SPA and web worker
crates/ports       port traits: SqlExecutor, Llm, Classifier, Mailer (services); MemberDirectory (mail-in)
crates/db          repositories over SqlExecutor; D1 backend (wasm only)
crates/llm         Llm/Classifier impls: Workers AI (allama-cloudflare-worker (chat + Clef decisions)) and `mock`
crates/agent       minimal durable agent loop + Tool trait
crates/services    ALL Restate services/VOs/workflows/agents, generic over ports; builds wasm32 + native
apps/web           Dioxus 0.7 SPA (Tailwind v4 + daisyUI 5, dioxus-daisyui-components, dioform, dioxus-clerk)
apps/web-worker    techdepo-web: axum /api/* + Static Assets (SPA)
apps/mail-in       techdepo-mail-in: #[event(email)]
apps/services      techdepo-services: Restate endpoint (request-response) on Workers
xtask              seed, catalog validate, dev-vars, evals (demo flows: scripts/smoke.sh, scenarios.sh)
data/catalog, data/kb, migrations, evals, deploy (compose.yaml)
```

## Hard-won facts (do not "fix" these)

- **`worker` is pinned to `=0.8.5`** (+ `worker-macros`): `worker >= 0.8.6` needs `wasm-bindgen >= 0.2.128`,
  but `dx 0.7.10` embeds wasm-bindgen 0.2.126. The workspace pins `wasm-bindgen = "=0.2.126"`.
- **`restate-sdk` comes from the fork** (`[patch.crates-io]`, branch `wasm`). Always `default-features = false`
  (workspace dep already does this). Never enable `http_server`.
- **Restate on Workers glue** is in `apps/services/src/lib.rs`: `Endpoint::handle_with_options(req,
  HandleOptions { protocol_mode: ProtocolMode::RequestResponse })`, collect the response body, return it.
  Verified working against local restate-server (register with `use_http_11: true`).
- **`restate-email` (email-rs) cannot be bound on Workers**: it enables restate-sdk default features
  (tokio net) and its transport futures are `!Send` on wasm while `ctx.run` needs `Send`. We implement our own
  `Email` Restate service in `crates/services` using `email-message` types; transports: Resend (HTTP) in prod,
  Mailpit HTTP API (`POST http://localhost:8025/api/v1/send`) locally.
- **Send bounds**: JS-backed futures (D1, Fetch, AI) are `!Send`. Wrap them in `send_wrapper::SendWrapper`
  (or `worker::send::SendFuture`) **inside the wasm adapters only** (`crates/db`, `crates/llm`, `apps/*`).
  Service code in `crates/services` must never see a JS type and must also `cargo check` natively.
- **Time**: never `std::time::SystemTime::now()` in handlers. Use `restate_ext::ContextClockExt` (`ctx.now()`).
  Pure code takes time as a parameter.
- Ingress invocation of a handler with no input: send **no body and no content-type**.
- **dx in this workspace**: always pass `-p techdepo-web` (dx 0.7.10 panics at `workspace.rs:325`
  resolving `default-members` relative to the cwd; `dx components add` has no `-p`, so run it in a
  scratch crate, see `apps/web/src/components/README.md`). The devenv's dx never downloads tools:
  it uses the `wasm-bindgen` on `PATH` (0.2.129) and refuses the mismatch with our 0.2.126 pin.
  `scripts/build-web.sh` puts a 0.2.126 first on `PATH` (worker-build's cache or a download).
- `[profile.wasm-release]` (root Cargo.toml) is dx's web release profile: `strip = "debuginfo"`,
  otherwise std's DWARF makes wasm-opt (binaryen 132) abort and the SPA ships unoptimized.
- SPA CSS is built by the Tailwind CLI into `apps/web/public/tailwind.css` (git-ignored, linked
  from `index.html`, not `asset!()`), so `cargo check` works without a CSS build.
- The SPA build output is `apps/web-worker/public` (git-ignored except `.gitkeep`): the web worker
  serves it as Static Assets and falls back to `index.html` only for unknown *extension-less*
  non-`/api` paths (`spa::fallback`; no `not_found_handling`, so misses reach the worker).
  A missing file (`/assets/foo.js`) is a 404, never the shell.
- **Persistence (`crates/db`)**: reads go through `Db::<repo>(&OrgScope)` (`orders`, `carts`,
  `invoices`, …); `techdepo_db::views` turns rows into `api_types` DTOs (cart pricing + compat
  report, demo panel, order/invoice/payment/shipment views) and is what both the web worker and the
  services use. Writes are `*_statement(s)` combined into one `Db::batch` (one D1 transaction).
  `SqliteExecutor` (feature `sqlite`, native) runs the same SQL on SQLite with the migrations:
  `cargo test -p techdepo-db` checks every repository against the real schema.
- **Services wiring**: `Services::new(Ports::new(sql, mailer), Config).bind_with(Endpoint::builder(),
  &policy::merged(overrides))` (`bind` = built-in policies). `Ports` also carries `models: Arc<dyn
  ports::Models>` (`Unconfigured` unless set; tests and evals use `services::ports::FixedModels`).
  `MemberDirectory` is not a services port: only mail-in uses it.
- **Model settings** (`domain::ModelSettings`, global `app_settings` row, the Muhaha page): one
  provider + model per task slot. Services never hold a fixed model: each invocation reads the
  settings in a journaled `ctx.run(models::read(..)).name(models::STEP)` (agent pipeline:
  `models::stepped`), then `models::chat`/`models::classifier` for its slot. Workflows read them
  once at the start of `run`. On Workers, `LlmConfig::registry(env, secret)` is the `Models`
  port: the `AI` binding plus Anthropic/OpenAI/Azure/Gemini/OpenRouter when their secrets
  (`ModelProvider::secrets`; Azure: `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_RESOURCE`) are set,
  over `techdepo_llm::WorkerFetch` (allama `HttpTransport` on the Fetch API; no reqwest in the
  wasm graph). The env only gives the defaults: `LLM_BACKEND`, and `AI_*` as model addresses
  (`ModelChoice::parse`: `azure:<deployment>`; a bare ID is Workers AI).
- **Operator access** (`Access::Operator`, `/api/admin/muhaha/*`): `org:admin` of an org with
  `OrgFeature::Muhaha`, otherwise `404` (`endpoint::check_access`). The `Muhaha` service acts
  across orgs through `Org/reset_demo`, `Org/clear_data`, `Org/reseed_stock`.
- **LLM (`crates/llm`, allama)**: `LlmConfig::from_lookup(env)` then `.build(&worker::Env)` (wasm;
  Workers AI needs the `AI` binding: `[env.dev]`/`[env.live]`, or locally `wrangler dev --env workers-ai`,
  which `scripts/dev.sh`/devenv pick when `.env` says `LLM_BACKEND=workers-ai`; any AI binding makes
  `wrangler dev` refuse to start without `CLOUDFLARE_API_TOKEN`, so it is never top-level) or
  `.with_providers(chat, Some(decisions))` (native evals; `None` = JSON-mode classification) gives
  `LlmPorts { llm, classifier }` (evals; the services worker uses `registry`, above).
  `allama-providers` (model addresses `anthropic:…`, `openai:…`, `azure:…`, `azure-responses:…`,
  `gemini:…`, `cloudflare:…`) lives in xtask only, one xtask feature per provider: the Workers build their
  providers in `crates/llm/src/worker.rs` over `WorkerFetch`. `LLM_BACKEND=mock` is `MockLlm` +
  `MockClassifier`: deterministic, enough for every demo scenario offline. Budgets:
  `UsageLedger::for_org(org, utc date of ctx.now())` is the agent's `Budget` (`llm_usage`).
- **Agent loop (`crates/agent`)** is Restate-free: services implement `agent::Runtime::step` as
  `ctx.run(..).name(name)` (`Json<T>`, retryable `StepError` → plain error, else terminal; checked to
  compile with `Send` handler futures). Tools are `Tool<Cx>`; the tenant scope lives in `Cx`.
- **Agent routes** (`crates/services/src/agents/`): `ProductAdvisor`/`OrderAssistant` services run
  `pipeline::answer<R: Runtime>` (agent → guardrails → one retry → judge → `record-trace`), so
  tests drive the whole pipeline natively with `techdepo_agent::Inline` over SQLite (seed a
  `tickets` row: `agent_runs` has a FK to it). Tools take `ToolCx<'_, R>` and read D1 only via
  `ToolCx::read` (journaled `tool-<n>-<tool>`, journal JSON, not row structs). Tool names and
  parameter spellings are what `techdepo_llm::MockLlm` calls; change both together. The KB is
  compiled in (`agents::kb`): a new `data/kb` file must be listed in its `kb!`.
- **Email**: callers say what happened via `Notifications/order` (one-way from inside `Order`) or
  `Notifications/ticket` (support templates, threading headers); only `Email/send` touches the
  `Mailer` port and writes `email_log`. The email-outage fault is `demo_faults.email_outage_until`
  (set when the confirmation fires it); `Email/send` fails retryably until then.
- **Support routes** (`crates/services/src/support/routes/`, `investigation.rs`): deterministic
  services `CancelOrder`/`WhereIsMyOrder`/`BillingIssue`/`RefundRequest` (listed in
  `contract::route_target`) report one `RouteOutcome`; IDs of the workflows they start derive from
  ticket + generation. `Carrier/open_trace` answers `open` for `carrier::TRACE_LATENCY` (15 s real
  time); `Investigation` polls it (`restate_ext::poll::Poller`) and talks to the ticket with
  `route_result` without a generation (offer = `AskCustomer` with a fresh awakeable per round).
- **Human tasks**: `HumanTasks/create` (internal) + `decide` (ingress) resolves the workflow's
  awakeable with `human_tasks::TaskResolution`; ticket-owned kinds go to `SupportTicket/task_decided`
  via `crate::support` (the seam the support pipeline fills, also `refund_finished`).
- **Support pipeline** (`crates/services/src/support/`): `SupportTicket` (key `<org>:<ticket>`) is
  the only writer of `tickets`/`ticket_messages` and the only sender of support mail (renders it,
  then `Notifications/ticket` one-way; AI text only via guardrails + reply policy). `Router`
  classifies (`ports::Classifier`) and starts routes by name: `<Route>/run(RouteRequest)`
  services listed in `support::contract::route_target` (an unlisted route escalates). Routes
  report one `RouteOutcome` to `SupportTicket/route_result`, echoing `generation` (`None` for
  workflows reporting later). `apps/services` builds the models from `LLM_BACKEND`
  (`techdepo_llm::LlmConfig`). mail-in parses with `mail-parser`, writes inbound `email_log` rows
  itself, and its `intake` runs natively (`cargo test -p techdepo-mail-in`); locally
  `POST :8789/cdn-cgi/handler/email?from=…&to=…` with a raw `.eml` triggers it. mail-in
  **fails closed on DMARC** (our MTA's `Authentication-Results` must say `dmarc=pass`); the local
  trigger adds none, so dev runs with `INBOUND_SENDER_AUTH=trust-from` (wrangler.toml `[vars]`,
  refused unless `AUTH_MODE=dev`).
- **Worker config** comes from env vars (`apps/services/src/settings.rs`; `RESTATE_CONFIG` JSON
  overrides per-service policies). `cargo run -p xtask -- dev-vars` writes `apps/*/.dev.vars`
  from `.env`. `WORKER_BUILD=--dev wrangler dev …` builds the services worker unoptimized.
  The services worker **fails closed**: without `RESTATE_IDENTITY_KEYS` (or an explicit
  `EMAIL_TRANSPORT`) it refuses to start unless `AUTH_MODE=dev` (its local `[vars]` default).
  A rejected call reaches ingress as `{"code":409,"message":"<ApiError JSON>"}`
  (`ingress::api_error_from_terminal`). `Order` is the only writer of `order_events`; others call
  `Order/record_event`.
- Handlers can be exercised natively: a scratch crate (outside the workspace, own `[workspace]`,
  same `[patch.crates-io]`, `restate-sdk` with `http_server`) serving `Services::bind` over
  `SqliteExecutor::open(path)`, registered with a **private** `restate` container on other ports
  (`-p 18080:8080 -p 19070:9070 --add-host=host.docker.internal:host-gateway`), so the shared one
  is not disturbed. Ingress-private services can be opened there with
  `PATCH /services/<name> {"public":true}`.
- **Web worker (`apps/web-worker`)**: every route of `endpoints::ALL` is mounted from its typed
  `Endpoint` (`routes/mod.rs` `mount!`; a test fails if one is missing) through
  `endpoint::{public, authed}`, which decode path/query/body and enforce `ROUTE.access`.
  Mutations go through `ingress::Ingress::{call, send}`; a terminal handler error passes through
  as its `ApiError`, ingress-level errors (`"source":"ingress"`, e.g. an unregistered service like
  `SupportTicket`) become `503 unavailable`. **Restate rejects `Idempotency-Key` on workflow
  handlers** (400): the client never sends one to `RefundWorkflow`/`Investigation` (the derived
  workflow key is the idempotency). Any authenticated request provisions an unknown org
  (`Org/provision`), not only `/api/me`.
- **Dev tokens** (`AUTH_MODE=dev`): `dev.<base64url(JSON DevClaims)>` (`auth::DevClaims`):
  `org_dev_<fnv1a64(lowercased org name):016x>`, `user_dev_<fnv1a64(lowercased email):016x>`.
- **Two `wrangler dev` processes sharing `--persist-to`** see transient `SQLITE_BUSY` on the local
  D1 ("database is locked", "Failed to get session commit token"). `D1Executor` classifies those
  as `SqlError::Unavailable` (Restate retries in services); the web worker and mail-in
  retry them (`techdepo_db::Retrying`). **`worker::Delay` panics in workerd** (it unwraps
  `setTimeout`'s return as a number); use a `setTimeout` promise (`techdepo_db::sleep`).
- **CI is GitHub Actions, not Dagger** (`dagger.toml`/`ci/` are unused): every job of
  `.github/workflows/ci.yaml` is one `just <recipe>` run in `devenv shell` (so `just` and every tool a
  recipe uses must come from `devenv.nix`), actions pinned by commit SHA with a `# vX.Y.Z` comment
  (bump them together; check with `git ls-remote https://github.com/<owner>/<repo> refs/tags/<tag>*`).
  `deny.toml` sets `allow-wildcard-paths` and `[bans.workspace-dependencies]
  include-path-dependencies = false`: cargo-deny cannot resolve the path-only `techdepo-*` workspace
  dependencies. Deploy = `scripts/deploy.sh --env dev|live` (manual `deploy.yaml` with an
  `environment` input, GitHub environments `dev`/`live`, `DEPLOY=true` variable; `docs/deploy.md`):
  it writes `apps/*/wrangler.deploy.toml` (no `[build]`, the env's D1 id) and passes per-environment
  values as `--var`s. Two deployments: `techdepo-dev-*` (shop on
  `techdepo-dev-web.vibekoders.workers.dev`) and `techdepo-live-*` (`techdepo.dev`), each with its
  own D1, Restate Cloud and Clerk. Their support local parts differ (`support-dev`, `support`):
  build support addresses from `SupportMailbox` (`api_types::session`; services:
  `Config::support_mailbox()`), never a literal `support+`.
- **Never return `Err` from a `#[event(email)]` handler**: worker-rs's glue panics on it, the panic
  aborts the wasm instance and every later delivery to that isolate fails with `unreachable`.
  mail-in exports `email` by hand (`apps/mail-in/src/glue.rs`) and rejects the promise instead.

## Commands

```bash
cargo check --workspace --exclude xtask --target wasm32-unknown-unknown   # everything that ships
cargo check -p techdepo-services                                           # native check of service code
cargo test -p techdepo-domain -p techdepo-catalog -p techdepo-compat -p techdepo-api-types -p techdepo-ports \
  -p techdepo-agent -p techdepo-services
cargo test -p techdepo-db                                                  # repositories on SQLite
cargo test -p xtask
cargo clippy --workspace --exclude xtask --target wasm32-unknown-unknown -- -D warnings
cargo clippy -p xtask --all-targets -- -D warnings                         # xtask is native only
cargo fmt --all -- --check
devenv up                                  # whole local stack in the foreground (devenv.nix: processes + tasks)
scripts/dev.sh --no-dx                     # whole local stack in the background (--help); scripts/dev-down.sh stops it
cargo run -p xtask -- catalog validate
cargo run -p xtask -- eval classify        # triage eval -> evals/reports/ (mock; --agent-model anthropic:… etc.)
cargo run -p xtask -- eval agents          # agent scenarios over a seeded SQLite shop (same options)
scripts/build-web.sh                       # SPA: Tailwind + dx release build -> apps/web-worker/public
scripts/build-web.sh --serve               # SPA dev server on :8090 (/api proxied to :8787)
docker compose -f deploy/compose.yaml up -d                                # restate (8080/9070) + mailpit (1025/8025)
cargo run -p xtask -- dev-vars                                           # apps/*/.dev.vars from .env
(cd apps/services && WORKER_BUILD=--dev wrangler dev --port 8788 --ip 0.0.0.0 --persist-to ../../.wrangler/state)
curl -X POST localhost:9070/deployments -H 'content-type: application/json' \
  -d '{"uri":"http://host.docker.internal:8788","use_http_11":true,"force":true}'
(cd apps/web-worker && WORKER_BUILD=--dev wrangler dev --port 8787 --ip 0.0.0.0 --persist-to ../../.wrangler/state)
scripts/smoke.sh                           # end-to-end /api flows against the running stack (dev auth)
scripts/scenarios.sh [1 2 … 11 policy screen]  # demo scenarios 1–11, AI reply policy, screening by email + Mailpit checks (~3 min; needs mail-in)
cargo test -p techdepo-web-worker          # route table coverage, auth, ingress error mapping
just fmt|check|clippy|test|deny|catalog|eval   # one CI job (justfile; .github/workflows/ci.yaml runs each)
just build-workers | just build-web        # CI's release builds (heavy: one at a time, never next to dx)
just ci                                    # every CI job in order, heavy builds last
scripts/deploy.sh --env dev|live [--no-build|--secrets|--dry-run]  # deploy (deploy/<env>.env); the Deploy workflow runs it
```

Ports: web worker 8787, services worker 8788, mail-in 8789, dx serve 8080→ use 8090 (Restate ingress owns 8080),
Restate admin 9070, Mailpit UI 8025.

**Memory budget (the machine froze twice):** 4 CPUs, ~8 GB RAM, **no swap**. `.cargo/config.toml` caps
`jobs = 2`. Never run two heavy builds at once (release/LTO `worker-build`, `scripts/build-web.sh`, `dx`),
prefer `cargo check -p <crate>`, use `WORKER_BUILD=--dev`, run at most one copy of each `wrangler dev`, and
close the headless browser and stop every background process (wrangler, workerd, dx) as soon as you are done.
**One heavy process at a time, ever:** never start `scripts/build-web.sh` (or any `dx`) while a `dx serve`/
`build-web.sh --serve` is running — stop the server first (`pkill -x dx`); never run `dagger` (the retired
Dagger CI in `dagger.toml`/`ci/` builds everything in parallel containers); close every Playwright/Chromium browser right after use
(`browser_close`, or kill the `chrome`/`headless_shell` processes you started) and never leave more than one
open; check `free -m` before a heavy step. A user-space guard (`~/.local/bin/memguard`, log in
`~/.local/state/memguard.log`) kills the biggest build/browser process if MemAvailable drops below 700 MB —
if your build or browser dies with SIGKILL, that is why: free memory and retry, do not parallelize.
`worker-build --release` takes minutes; use `worker-build --dev` while iterating.
**Disk:** `target/` grows past 50 GB (native + wasm32 profiles, feature sets per command) and once filled
the 79 GB disk mid-build ("No space left on device"): check `df -h /` before big builds; deleting
`target/*/incremental` and `target/wasm32-unknown-unknown/*/incremental` is safe and frees ~15 GB.

## Conventions

- Edition 2024, `thiserror` for errors, `serde` everywhere, no `unwrap()` in handlers.
- Every D1 table carries `org_id` and every query filters on it (`OrgScope`), except global catalog data
  and `app_settings` (the operator's, written only by `Muhaha`).
- Every state change goes through a Restate handler; the web worker reads D1 directly but never writes
  business state itself.
- Money is `i64` cents EUR, VAT-inclusive; VAT rate per org (default 27%).
- Local dev runs without accounts: `LLM_BACKEND=mock`, `AUTH_MODE=dev` (dev login, never deployed),
  `EMAIL_TRANSPORT=mailpit`.
- Match the surrounding code: comment density, naming, idiom. Keep modules focused.
- Do not commit; the orchestrator commits after each milestone.
