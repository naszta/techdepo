# Running TechDepó locally

The whole stack runs on one machine without any account: dev sign-in (`AUTH_MODE=dev`), a
deterministic mock LLM (`LLM_BACKEND=mock`) and Mailpit for email (`EMAIL_TRANSPORT=mailpit`).
Real Workers AI, real Clerk sign-in and the eval models are opt-in.

## Prerequisites

- [devenv](https://devenv.sh) (Nix). Its shell (`devenv.nix`) provides the Rust toolchain with
  `wasm32-unknown-unknown`, `worker-build`, `dx`, the right `wasm-bindgen`, node, wrangler, `just`
  and `jq`.
- Docker, for Restate and Mailpit (`deploy/compose.yaml`).
- About 8 GB of RAM and 60 GB of disk for `target/`. Builds run one at a time, and cargo is
  capped at two jobs (`.cargo/config.toml`).

## Quick start

```bash
cp .env.example .env     # optional: the defaults are the local ones
devenv up                # the whole stack in the foreground (Ctrl-C stops it)
```

Or in the background:

```bash
scripts/dev.sh --no-dx   # Restate + Mailpit, .dev.vars, migrations, SPA build, the three workers
scripts/dev-down.sh      # stop everything
```

The first run builds everything and takes several minutes. Then:

| URL | What |
|---|---|
| <http://localhost:8787> | the shop, the customer account and the admin area. **Sign in** with any shop name and email; the same shop name brings you back to the same shop. |
| <http://localhost:8090> | `dx serve`: the SPA with hot reload (`/api` proxied to :8787), unless `--no-dx` |
| <http://localhost:8025> | Mailpit: every email the shop sends |
| <http://localhost:9070> | the Restate UI (invocations, journals, state) |

| Port | Process |
|---|---|
| 8787 | `techdepo-web` (`wrangler dev`): API + the SPA build |
| 8788 | `techdepo-services` (`wrangler dev`): the Restate endpoint, registered with the local Restate |
| 8789 | `techdepo-mail-in` (`wrangler dev`): inbound email |
| 8080 / 9070 | Restate ingress / admin |
| 1025 / 8025 | Mailpit SMTP / UI and HTTP API |

`scripts/dev.sh --help` lists the options: `--restart web|services|mail-in|dx`, `--build-web`,
and `--release` for optimized worker builds. Logs go to `.wrangler/dev/logs/<name>.log`. All
three workers share the local D1 in `.wrangler/state`.

## Configuration

- The root **`.env`** (git-ignored; template [`.env.example`](../.env.example)) is the local
  configuration. devenv loads it automatically.
- `cargo run -p xtask -- dev-vars` writes each worker's `apps/*/.dev.vars` with only the keys that
  worker reads. `devenv up` and `scripts/dev.sh` run it for you; run it yourself after
  editing `.env` while the stack is up, then restart the affected worker.
- Each `wrangler.toml`'s top-level `[vars]` holds the local defaults. The `[env.dev]` and
  `[env.live]` tables are the deployed environments ([deploy.md](deploy.md)) and are never
  used locally.

## Trying the flows

```bash
scripts/smoke.sh                      # end-to-end /api flows (dev auth)
scripts/scenarios.sh                  # the eleven demo scenarios + Mailpit checks (~3 min)
scripts/scenarios.sh 4 policy         # some of them
```

Inbound email, without Email Routing:

```bash
# wrangler's local email trigger (any raw .eml)
curl -X POST 'localhost:8789/cdn-cgi/handler/email?from=jane@example.com&to=support+<org>@techdepo.dev' \
  --data-binary @mail.eml
```

The in-app **Send as email** form (Account → Support) goes through the same intake. The support
address `support+<org>@techdepo.dev` is shown under Account. Local mail-in trusts the From
address (`INBOUND_SENDER_AUTH=trust-from`, dev only). Deployed environments require
`dmarc=pass`.

## Real models

### In the services worker

The `.env` values below are the **defaults**. Which model answers each AI task is chosen on the
[Muhaha page](#the-muhaha-page), per task, from the providers the services worker can reach.

#### Workers AI

Workers AI is reached through the `AI` binding. To use it locally, set this in `.env`:

```bash
LLM_BACKEND=workers-ai
CLOUDFLARE_ACCOUNT_ID=…
CLOUDFLARE_API_TOKEN=…          # Workers AI:Read is enough locally
AI_AGENT_MODEL=@cf/meta/llama-3.3-70b-instruct-fp8-fast   # optional
AI_CLASSIFIER_MODEL=@cf/cloudflare/clef                   # optional
```

The models are addresses, `<provider>:<model>` (`anthropic:claude-sonnet-5`,
`azure-responses:<deployment>`); a bare model ID is Workers AI.

`devenv up` and `scripts/dev.sh` then start the services worker with `wrangler dev --env
workers-ai`, a remote AI binding (inference runs on your account). It isn't the default because
wrangler refuses to start a worker that declares an AI binding unless a token is set.

#### Anthropic, OpenAI, Azure OpenAI, Gemini, OpenRouter

Put the key in `.env` (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`,
`OPENROUTER_API_KEY`; Azure OpenAI needs `AZURE_OPENAI_API_KEY` and `AZURE_OPENAI_RESOURCE`, the
resource's name), run `cargo run -p xtask -- dev-vars` and restart the services worker. The
provider is then selectable on the Muhaha page (Azure as `azure`, Chat Completions, and
`azure-responses`, the Responses API; model IDs are deployment names). `LLM_BACKEND` can stay `mock`: the mock and these
providers need no Cloudflare account.

### The Muhaha page

The operator's page (`/admin/muhaha`: model settings, and resetting, clearing or reseeding any
shop) is only for admins of an org with the `muhaha` feature. Turn it on for your local org once
it exists (sign in first; the org ID is under Account):

```bash
(cd apps/services && npx wrangler@4 d1 execute techdepo --local --persist-to ../../.wrangler/state \
  --command "UPDATE orgs SET features_json = '[\"muhaha\"]' WHERE id = 'org_dev_…'")
```

Reload the app: Muhaha appears in the admin sidebar, under Operator. Saved model settings live in
the local D1 and apply to new work; running workflows keep their models.

### In the evals: any provider

The evals run the router's triage and the agent pipeline natively. They can reach **Anthropic,
OpenAI, Azure OpenAI, Gemini and Cloudflare** (Workers AI over HTTP) through `allama-providers`.
These are xtask's cargo features, all on by default (the Workers build their own providers, see
above; Cloudflare over HTTP exists only here).

```bash
cargo run -p xtask -- eval classify                                     # mock models
cargo run -p xtask -- eval agents --backend workers-ai                  # AI_AGENT_MODEL / AI_CLASSIFIER_MODEL on Cloudflare
cargo run -p xtask -- eval agents --agent-model anthropic:claude-haiku-4-5 --classifier-model none
cargo run -p xtask -- eval classify --agent-model openai:gpt-5-mini \
  --classifier-model cloudflare:@cf/cloudflare/clef
cargo run -p xtask -- eval agents --agent-model gemini:gemini-2.5-flash --classifier-model none
cargo run -p xtask -- eval agents --agent-model azure:<deployment> --classifier-model none
cargo run -p xtask -- eval agents --agent-model azure-responses:<deployment> --classifier-model none
```

- Models are model addresses, `<provider>:<model>`. A bare `@cf/…` ID means `cloudflare:`.
- `--classifier-model` takes a decision model (Clef on `cloudflare:`). `none` classifies in JSON
  mode with the agent model, so the run needs only that provider's key.
- Keys come from the environment or `.env`:

  | Provider | Variables |
  |---|---|
  | `anthropic` | `ANTHROPIC_API_KEY` |
  | `openai` | `OPENAI_API_KEY` |
  | `azure`, `azure-responses` | `AZURE_OPENAI_API_KEY`, and `AZURE_OPENAI_ENDPOINT` or `AZURE_OPENAI_RESOURCE` |
  | `gemini` | `GEMINI_API_KEY` |
  | `cloudflare` | `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_API_TOKEN` |

- Reports go to `evals/reports/<eval>-<provider>-<model>.md`. `--only <id>` and `-v` help while
  iterating. Fewer providers: `cargo run -p xtask --no-default-features --features anthropic -- …`.

## Real sign-in (Clerk)

With a Clerk development instance (Organizations on, membership required), set this in `.env`:

```bash
AUTH_MODE=clerk
CLERK_PUBLISHABLE_KEY=pk_test_…
CLERK_SECRET_KEY=sk_test_…
CLERK_AUTHORIZED_PARTIES=http://localhost:8787,http://localhost:8090
```

Then restart the stack. Only the web and mail-in workers get this `AUTH_MODE`. The services
worker always runs locally with `AUTH_MODE=dev` from its own `[vars]`. For that worker, the
setting has nothing to do with sign-in: it lets it accept the unsigned local Restate.

## Step by step, without devenv up

What `scripts/dev.sh` does, if you want to run the parts yourself (in the devenv shell):

```bash
docker compose -f deploy/compose.yaml up -d                   # restate (8080/9070) + mailpit (1025/8025)
cargo run -p xtask -- dev-vars                                # apps/*/.dev.vars from .env
(cd apps/services && npx wrangler@4 d1 migrations apply techdepo --local --persist-to ../../.wrangler/state)
(cd apps/services && WORKER_BUILD=--dev wrangler dev --port 8788 --ip 0.0.0.0 --persist-to ../../.wrangler/state)
curl -X POST localhost:9070/deployments -H 'content-type: application/json' \
  -d '{"uri":"http://host.docker.internal:8788","use_http_11":true,"force":true}'
scripts/build-web.sh                                          # SPA -> apps/web-worker/public
(cd apps/web-worker && WORKER_BUILD=--dev wrangler dev --port 8787 --ip 0.0.0.0 --persist-to ../../.wrangler/state)
(cd apps/mail-in && WORKER_BUILD=--dev wrangler dev --port 8789 --ip 0.0.0.0 --persist-to ../../.wrangler/state)
scripts/build-web.sh --serve                                  # optional: dx serve on :8090
```

Start one at a time: each `wrangler dev` builds its worker first.

## Checks

The CI jobs are `just` recipes, and each runs the same way locally:

```bash
just fmt check clippy test     # formatting, wasm32 + native checks, clippy, tests
just deny catalog eval         # cargo-deny, catalog validation, mock evals
just build-workers build-web   # release builds (heavy; one at a time, never next to dx)
just ci                        # all of them, heavy builds last
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| A build or the browser dies with SIGKILL | The memory guard killed it (MemAvailable below 700 MB). Stop `dx`, close extra browsers, and retry without parallel builds. |
| "No space left on device" | `target/` is huge: delete `target/*/incremental` and `target/wasm32-unknown-unknown/*/incremental`. |
| `database is locked` / `SQLITE_BUSY` in logs | Two workers share the local D1. It's transient and retried. |
| dx refuses the wasm-bindgen version | Use `scripts/build-web.sh`: it puts the pinned 0.2.126 first on `PATH`. |
| Mutations answer 503 | The services worker isn't registered with the local Restate: re-run the `curl … /deployments` above, or `scripts/dev.sh --restart services`. |
| The SPA doesn't change after a rebuild | `wrangler dev` caches assets: `scripts/dev.sh --restart web`. |
| Clerk says "The External Account was not found" | A new social account whose sign-in Clerk could not turn into a sign-up: sign-ups are blocked, or the `SignIn` route lost its `#:clerk` hash ([deploy guide](deploy.md#troubleshooting)). |
