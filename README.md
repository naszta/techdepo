# TechDepó

A demo webshop for PC parts that shows **what should be code, what should be a durable workflow,
what should be an AI agent, and when a person must decide**.

It looks and behaves like a real shop: a catalog of fictional-brand parts on real standards, a
compatibility checker that is always right, a PC builder, checkout, orders, invoices and a support
inbox. Each organization gets its own isolated shop. After placing an order you play the outside
world (pay, ship, move the clock, charge twice, lose the parcel, take the email provider down),
email support and watch workflows, agents and human approvals handle it.

Nothing is real: no money moves and nothing ships.

Built in Rust on Cloudflare Workers (a Dioxus SPA, an axum API, an email worker), with
[Restate](https://restate.dev) for durable execution and Workers AI for classification and agents.

## Code, workflow, agent or person

Every step in the shop is one of four things, on purpose:

| | When | In this demo |
|---|---|---|
| **Code** | The rule is known | Compatibility checker, shipping policy, duplicate-charge detector, invoice numbering |
| **Durable workflow** | It takes time, can fail and must finish | Order lifecycle, refunds, lost-parcel investigations, email retries |
| **AI agent** | The steps can't be known up front | Product advice, open-ended order questions |
| **A person** | Money moves or confidence is low | Approving refunds, refund or reship, vague or angry email, reviewing AI drafts |

A support email shows all four. mail-in identifies the sender and the shop. A classifier (Workers
AI Clef) scores the intent with real per-option probabilities, and the router compares them with
a confidence threshold. A confident "where is my order?" goes to a deterministic route: the
shipping policy, in workdays and in code, decides between "standard answer", "please wait" and
opening a lost-parcel investigation. That investigation is a Restate workflow that polls the
carrier, asks a person to offer a refund or a reship, waits durably for the customer's reply
(an LLM only interprets it: accept refund, accept reship, decline, unclear), and refunds or
places a store-paid replacement order. Only open-ended questions ("will these parts work
together?") reach an agent, which has read-only tools, is grounded in the compatibility engine,
and passes guardrails and a judge. Code sends every email, under a per-shop AI reply policy.
Anything unclear is escalated to a person with the full context.

The design is in [the plan](docs/plan/README.md), the support side in
[support.md](docs/plan/support.md).

## Architecture

```
                   ┌──────────────────────────── Cloudflare ─────────────────────────────┐
 Browser ─ HTTPS ─►│ techdepo-web      Dioxus SPA (Static Assets) + /api/* (axum).        │
                   │                   Reads D1; every mutation → Restate ingress         │
 Customer mail ───►│ Email Routing ─► techdepo-mail-in   parse, verify sender, dedupe     │
                   │                                      → Restate ingress               │
 Restate Cloud ───►│ techdepo-services  one Restate endpoint: every service, virtual      │
  (signed calls)   │                    object, workflow and agent; D1, Workers AI, email │
                   │ D1 (techdepo)                 Workers AI                             │
                   └─────────────────────────────────────────────────────────────────────┘
          web, mail-in ── ingress ──► Restate Cloud ──► techdepo-services
          techdepo-services ──► Resend (outbound mail)      web, mail-in ──► Clerk (auth)
```

- **techdepo-web** serves the UI and the API and never runs long work: reads go straight to D1,
  every state change is a Restate handler, so it is journaled, retried and visible in the
  Restate UI.
- **techdepo-mail-in** is a thin adapter: no AI and no business logic beyond identification.
- **techdepo-services** hosts everything durable: `Order` (a virtual object state machine),
  refunds and investigations (workflows), support tickets (virtual objects), human tasks
  (awakeables), the agents and our own `Email` service.

Details: [architecture.md](docs/plan/architecture.md).

## Run it locally

Everything runs without accounts: dev sign-in (`AUTH_MODE=dev`), a deterministic mock LLM
(`LLM_BACKEND=mock`) and Mailpit for email (`EMAIL_TRANSPORT=mailpit`). The
[devenv](https://devenv.sh) shell (`devenv.nix`) provides the toolchain; Docker runs Restate and
Mailpit.

```bash
cp .env.example .env     # optional: the defaults are the local ones
devenv up                # the whole stack in the foreground (Ctrl-C stops it)
# or, in the background:
scripts/dev.sh --no-dx   # Restate + Mailpit, migrations, SPA build, the three workers
scripts/dev-down.sh      # stops it
```

The first build takes several minutes (the machine needs about 8 GB of RAM; builds run one at a
time). Then open:

- <http://localhost:8787>: the shop, the customer account and the admin area. **Sign in** with any
  shop name and email; the same shop name brings you back to the same shop.
- <http://localhost:8025>: Mailpit, every email the shop sends.
- <http://localhost:9070>: the Restate UI.

`scripts/smoke.sh` and `scripts/scenarios.sh` exercise the running stack end to end. The mock LLM
is keyword based; set `LLM_BACKEND=workers-ai` (and the Cloudflare token) in `.env` for real
models. [docs/local-development.md](docs/local-development.md) has the details: ports,
configuration, real models and sign-in, the evals on other providers, troubleshooting.

## Demo scenarios

Eleven scenarios double as acceptance tests ([roadmap.md](docs/plan/roadmap.md#demo-scenarios-these-double-as-acceptance-tests)):
charged twice, an invoice without an address, "where is my order?" across simulated workdays, a
lost parcel with a refund-or-reship decision, declined offers, cancelling unpaid, paid and shipped
orders, a missing order number, a compatibility question for the agent, vague or angry email,
prompt injection, and an email provider outage.

- [Presenter guide](docs/presenter-guide.md): click paths and what to say, scenario by scenario.
- `scripts/scenarios.sh [1 2 … 11]`: runs them against the local stack (about 3 minutes).
- `/demo` in the running app: the "how this demo works" page for visitors.

## Configuration

Local configuration is the root `.env` (template: [.env.example](.env.example)); `cargo run -p
xtask -- dev-vars` writes each worker's `.dev.vars` from it (devenv and `scripts/dev.sh` do this
for you). The deployed environments need Cloudflare (Workers Paid, D1, Email Routing, Workers
AI), Restate Cloud, Clerk and Resend: [docs/deploy.md](docs/deploy.md) has the setup.

## CI and deployment

- [CI](.github/workflows/ci.yaml) runs on every push to `main` and every pull request: one job per
  [justfile](justfile) recipe, each in the devenv shell (`fmt`, `check`, `clippy`, `test`, `deny`,
  `catalog`, `eval`, `build-workers`, `build-web`). `just ci` runs them all locally.
- [Deploy](.github/workflows/deploy.yaml) is manual, per environment: **dev**
  (<https://techdepo-dev-web.vibekoders.workers.dev>, `techdepo-dev-*` Workers) or **live**
  (<https://techdepo.dev>, `techdepo-live-*`, from `main` only). It is off until the repository
  variable `DEPLOY` is `true`. It runs [scripts/deploy.sh](scripts/deploy.sh) `--env <dev|live>`:
  D1 migrations, the services worker as a new version whose preview URL is registered with the
  environment's Restate Cloud before it takes traffic, then the web and mail-in workers. The
  script works from a shell too, with the values in `deploy/<env>.env`.
  [docs/deploy.md](docs/deploy.md) covers the accounts, workers, secrets and models.

## Project layout

```
crates/domain        pure types and rules: ids, money, order state machine, workdays, shipping policy
crates/catalog       typed product specs and the catalog compiled in from data/catalog
crates/compat        the compatibility rules engine (shared by the SPA, the API and the agents)
crates/api-types     /api/* DTOs shared by the SPA and the web worker
crates/ports         traits the services depend on (SQL, LLM, classifier, mailer, member directory)
crates/db            repositories over the SQL port; D1 backend (wasm) and SQLite (tests)
crates/llm           Workers AI (allama) and the deterministic mock
crates/agent         a minimal durable agent loop and its Tool trait
crates/services      every Restate service, virtual object, workflow and agent (wasm32 and native)
apps/web             the Dioxus SPA (Tailwind v4 + daisyUI 5)
apps/web-worker      techdepo-web: axum /api/* + the SPA as Static Assets
apps/mail-in         techdepo-mail-in: inbound email
apps/services        techdepo-services: the Restate endpoint on Workers
xtask                catalog validation, seeding, dev-vars, evals
data/                catalog and knowledge base; migrations/ (D1); evals/; deploy/ (local compose,
                     deploy env template)
docs/                the plan (the spec), deploy and local development guides, presenter guide
```

[CLAUDE.md](CLAUDE.md) has the commands and the hard-won facts for contributors.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
