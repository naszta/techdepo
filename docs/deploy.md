# Deploying TechDepó

TechDepó runs as two independent deployments of the same build. Each one has its own Workers, D1
database, Restate Cloud environment, Clerk instance and support address. Everything
environment-specific is configuration, so one commit can go to dev first and then, unchanged, to
live.

| | **dev** | **live** |
|---|---|---|
| Shop | <https://techdepo-dev-web.vibekoders.workers.dev> | <https://techdepo.dev> |
| Workers | `techdepo-dev-web`, `techdepo-dev-services`, `techdepo-dev-mail-in` | `techdepo-live-web`, `techdepo-live-services`, `techdepo-live-mail-in` |
| D1 | `techdepo-dev` | `techdepo-live` |
| Restate Cloud | its own environment (e.g. `techdepo-dev`) | its own environment (e.g. `techdepo-live`) |
| Clerk | development instance | production instance on `techdepo.dev` |
| Support address | `support-dev+<org>@techdepo.dev` | `support+<org>@techdepo.dev` |
| Demo orgs expire after | 7 days | 30 days |
| GitHub environment | `dev` | `live` (from `main` only) |

The non-secret per-environment values are committed in the `[env.dev]` and `[env.live]` tables
of `apps/*/wrangler.toml`. Secrets, IDs and account URLs live in GitHub environments (CI) or in
`deploy/<env>.env` (shell).

## How a deploy works

`scripts/deploy.sh --env <dev|live>` (run by `.github/workflows/deploy.yaml`):

1. **Builds** the SPA and the three Workers in release mode, one at a time.
2. **Applies the D1 migrations** to the environment's database (`wrangler d1 migrations apply DB
   --remote`). Migrations are additive, so the code that is still running keeps working.
3. **Uploads the services worker as a new version** (`wrangler versions upload`). It doesn't
   take traffic yet. Every version gets its own immutable **preview URL**
   (`https://<version>-techdepo-<env>-services.vibekoders.workers.dev`).
4. **Registers that preview URL with the environment's Restate Cloud** (`POST
   /deployments`, HTTP/1.1). Restate pins every invocation to the deployment that started it,
   so in-flight workflows finish on the code they started on, while new invocations go to the
   new version. Separate environments, and separate versions within one, never share a URL.
5. **Promotes the version** to 100% (`wrangler versions deploy`).
6. **Deploys the web and mail-in workers** with their per-environment `--var`s.

Restate Cloud signs every request it sends to the services worker. The worker checks the
signature against `RESTATE_IDENTITY_KEYS` and **refuses to start without them**. Preview URLs
are public, so without the keys anyone could call any handler, internal ones included. That
is why the services worker needs its secrets before the first registration (see
[First deploy](#first-deploy-of-an-environment)).

## One-time setup

Do this once per account, then once per environment where marked.

### Cloudflare

1. Use the **Workers Paid** plan. The Free plan's CPU and bundle limits don't fit Rust and
   Restate.
2. **workers.dev subdomain:** Workers & Pages → Settings → *Subdomain* = `vibekoders`. Dev's shop
   and every services preview URL live under it.
3. **API token** (My Profile → API Tokens → Create Token → Custom) with these permissions:
   - Account · Workers Scripts · Edit
   - Account · D1 · Edit
   - Account · Workers AI · Read
   - Zone · Workers Routes · Edit, and Zone · DNS · Edit, on `techdepo.dev` (for live's custom
     domain)

   It becomes → `CLOUDFLARE_API_TOKEN`. The account ID (dashboard sidebar) → `CLOUDFLARE_ACCOUNT_ID`.
4. **D1, per environment:**

   ```bash
   npx wrangler@4 d1 create techdepo-dev    # → D1_DATABASE_ID of dev
   npx wrangler@4 d1 create techdepo-live   # → D1_DATABASE_ID of live
   ```

   The committed `wrangler.toml` files hold a placeholder ID. The deploy substitutes the
   environment's `D1_DATABASE_ID`.
5. Workers AI needs no setup: the services worker uses the `AI` binding.

### Restate Cloud (per environment)

Create two environments, e.g. `techdepo-dev` and `techdepo-live`, in the same region. For each:

| Where | Value | Becomes |
|---|---|---|
| Environment overview | ingress URL, `https://<env>.env.<region>.restate.cloud:8080` | `RESTATE_INGRESS_URL` |
| Environment overview | admin URL, `https://<env>.env.<region>.restate.cloud:9070` | `RESTATE_ADMIN_URL` |
| Developers → API keys | key with the **Ingress** role (the web and mail-in workers call handlers) | `RESTATE_API_KEY` |
| Developers → API keys | key with the **Admin** role (the deploy registers deployments) | `RESTATE_ADMIN_TOKEN` |
| Developers → Security → HTTP endpoints | request identity public key, `publickeyv1_…` | `RESTATE_IDENTITY_KEYS` |

Never mix the two environments' values. Live's services worker must only accept live's
signatures.

### Clerk (per environment)

- **dev:** a **development instance**. It works on `workers.dev` without DNS.
- **live:** a **production instance** for `techdepo.dev`. Add the DNS records Clerk lists
  (`clerk.`, `accounts.`, and the email CNAMEs) in the `techdepo.dev` zone.

On both instances:

- Turn on **Organizations**, with **membership required**.
- Sign-up mode: open while the demo is public.
- API keys → `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`.
- `CLERK_AUTHORIZED_PARTIES` (the allowed `azp` origins) defaults to the environment's shop URL.
  Set it only to allow more origins.

### Email (Cloudflare Email Routing and Resend)

Both environments share the `techdepo.dev` zone, with a different support local part each.

1. **Email Routing** on `techdepo.dev`: enable it (it adds the MX records), then Settings →
   **Subaddressing** on.
2. After each environment's first deploy (the worker must exist), add its routing rule:
   - `support@techdepo.dev` → *Send to a Worker* → `techdepo-live-mail-in`
   - `support-dev@techdepo.dev` → *Send to a Worker* → `techdepo-dev-mail-in`

   With subaddressing, `support+<org>@` and `support-dev+<org>@` follow their rule.
3. **Resend:** add the domain `techdepo.dev` with the `send.techdepo.dev` return path. Add DKIM
   (`resend._domainkey`), the return-path MX and SPF on `send.`, one merged SPF on the apex
   (together with Email Routing's include), and DMARC on the apex. Create one sending API key
   per environment → `RESEND_API_KEY`.

### GitHub (per environment)

Settings → Environments → create `dev` and `live`. Repository-level secrets apply to both
environments, so the Cloudflare values can live there once.

| Name | Kind | Scope | Notes |
|---|---|---|---|
| `CLOUDFLARE_ACCOUNT_ID` | secret | repository | |
| `CLOUDFLARE_API_TOKEN` | secret | repository | |
| `D1_DATABASE_ID` | secret | environment | `techdepo-dev` / `techdepo-live` |
| `RESTATE_ADMIN_URL` | secret | environment | |
| `RESTATE_ADMIN_TOKEN` | secret | environment | |
| `RESTATE_IDENTITY_KEYS` | secret | environment | uploaded with the `secrets` input |
| `RESTATE_API_KEY` | secret | environment | uploaded with the `secrets` input |
| `RESEND_API_KEY` | secret | environment | uploaded with the `secrets` input |
| `CLERK_SECRET_KEY` | secret | environment | uploaded with the `secrets` input |
| `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY`, `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_RESOURCE` | secret | environment | optional, uploaded with the `secrets` input, see [Models](#models) |
| `RESTATE_INGRESS_URL` | variable | environment | |
| `CLERK_PUBLISHABLE_KEY` | variable | environment | |
| `CLERK_AUTHORIZED_PARTIES` | variable | environment | optional |
| `LLM_BACKEND`, `AI_AGENT_MODEL`, `AI_CLASSIFIER_MODEL`, `AI_FALLBACK_CLASSIFIER`, `LLM_DAILY_TOKEN_BUDGET_PER_ORG`, `LLM_DAILY_TOKEN_BUDGET_GLOBAL` | variable | environment | optional, see [Models](#models) |
| `DEPLOY` = `true` | variable | repository | enables the workflow |

Protect `live`: add required reviewers and limit deployment branches to `main`. The workflow also
refuses a live deploy from any other ref.

## Workers and their secrets

The workers are named in `wrangler.toml` (`[env.<env>] name`). Cloudflare creates a Worker the
first time anything is uploaded to it. Creating them by putting their secrets is the clearest
way, because the services worker must have `RESTATE_IDENTITY_KEYS` before Restate first calls it.

| Worker | Secrets |
|---|---|
| `techdepo-<env>-services` | `RESTATE_IDENTITY_KEYS`, `RESEND_API_KEY`, `CLERK_SECRET_KEY`; optional model provider keys ([Models](#models)) |
| `techdepo-<env>-web` | `CLERK_SECRET_KEY`, `RESTATE_API_KEY` |
| `techdepo-<env>-mail-in` | `CLERK_SECRET_KEY`, `RESTATE_API_KEY` |

By hand, from the repository root, for `ENV=dev` (then `ENV=live`). Each command prompts for the
value. The first command on a Worker that doesn't exist yet offers to create it: answer yes.

```bash
ENV=dev
export CLOUDFLARE_ACCOUNT_ID=… CLOUDFLARE_API_TOKEN=…

(cd apps/services &&
  for key in RESTATE_IDENTITY_KEYS RESEND_API_KEY CLERK_SECRET_KEY; do
    npx wrangler@4 secret put "$key" --env "$ENV"
  done)
(cd apps/web-worker &&
  for key in CLERK_SECRET_KEY RESTATE_API_KEY; do npx wrangler@4 secret put "$key" --env "$ENV"; done)
(cd apps/mail-in &&
  for key in CLERK_SECRET_KEY RESTATE_API_KEY; do npx wrangler@4 secret put "$key" --env "$ENV"; done)
```

The alternative is to keep the secrets in the GitHub environment and run the workflow with
**secrets** checked (`scripts/deploy.sh --secrets`). That uploads every non-empty one with the new
versions. Uploads are additive: a secret left out keeps its value. Run with secrets checked
whenever you rotate one.

## Models

The operator picks the model of each AI task (product advisor, order assistant, triage, offer
replies, judge, classifier fallback) on the **Muhaha page** (`/admin/muhaha`, see
[the operator's page](#the-operators-page-muhaha)). That choice is stored in the environment's D1
(`app_settings`), survives deploys, and applies to work that starts after it is saved: running
workflows keep the models they started with.

**Providers.** Workers AI through the `AI` binding is always there. Anthropic, OpenAI, Azure
OpenAI, Gemini and OpenRouter become selectable once their secrets are set on the services worker;
their calls go out through the Workers Fetch API. Without them the provider shows as "not
configured" and cannot be saved.

| Secret (services worker) | Provider |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic |
| `OPENAI_API_KEY` | OpenAI (Responses API) |
| `AZURE_OPENAI_API_KEY` and `AZURE_OPENAI_RESOURCE` | Azure OpenAI: `azure` (Chat Completions) and `azure-responses` (Responses API). The resource is its name (`https://<resource>.openai.azure.com`), model IDs are deployment names |
| `GEMINI_API_KEY` | Google Gemini |
| `OPENROUTER_API_KEY` | OpenRouter (many vendors' models behind one key) |

Put them like the other secrets ([above](#workers-and-their-secrets)), or as secrets of the GitHub
environment and deploy with **secrets** checked. Classification is calibrated only on a decision
model (Workers AI Clef); any other model classifies in JSON mode with self-reported confidence.

**Defaults.** Until settings are saved (and after "Use the deployment's defaults"), the models
come from variables of the services worker. Unset ones fall back to the defaults in
`crates/llm/src/config.rs`:

| Variable | Default | Meaning |
|---|---|---|
| `LLM_BACKEND` | `workers-ai` (wrangler.toml) | `workers-ai` (the models below) or `mock` (also shown in the UI by the web worker) |
| `AI_AGENT_MODEL` | `@cf/meta/llama-3.3-70b-instruct-fp8-fast` | the agents' function-calling model |
| `AI_CLASSIFIER_MODEL` | `@cf/cloudflare/clef` | the decision model for triage, offers and the judge |

The models are addresses, `<provider>:<model>` with the provider keys of the Muhaha page:
`anthropic:claude-sonnet-5`, `azure:<deployment>`, `azure-responses:<deployment>`. A bare model
ID is Workers AI (`cloudflare:` works too, as in the evals). The services worker refuses to start
when a default model's provider is not configured.
| `AI_FALLBACK_CLASSIFIER` | `on` | JSON-mode classification on the agent model when the decision model fails |
| `LLM_DAILY_TOKEN_BUDGET_PER_ORG` / `_GLOBAL` | `200000` / `5000000` | daily token budgets (`0` = unlimited) |

Set them as variables of the GitHub environment (or in `deploy/<env>.env`), and the next deploy
passes them as `--var`s. Try a model with the evals first. They run natively and can also reach
Anthropic, OpenAI, Azure OpenAI, Gemini and Cloudflare ([local development](local-development.md#real-models)).

## The operator's page (Muhaha)

`/admin/muhaha` holds the operator's tools: the model settings, and every shop of the
environment with **Reset every shop**, and per shop **Clear data** and **Reseed stock** (which
also gives an org the products added to the catalog since it was seeded). They act across
tenants, so only admins of an org with the `muhaha` feature see the page or reach its API; for
everyone else it does not exist (404). Turn the feature on for your own org by hand, once per
environment, after its first sign-in has provisioned it (the org ID is under Account, or in
`SELECT id, name FROM orgs`):

```bash
export CLOUDFLARE_ACCOUNT_ID=… CLOUDFLARE_API_TOKEN=…   # D1:Edit
npx wrangler@4 d1 execute "techdepo-$ENV" --remote \
  --command "UPDATE orgs SET features_json = '[\"muhaha\"]' WHERE id = 'org_…'"
```

Sign in again (or reload) to see Muhaha in the admin sidebar, under Operator.

## Deploying

**From GitHub:** Actions → **Deploy** → *Run workflow* → pick the branch (`main` for live), the
environment and, if needed, **secrets**. Runs for the same environment queue instead of
overlapping.

**From a shell** (devenv shell, so every tool is there):

```bash
cp deploy/env.example deploy/dev.env     # fill in; git-ignored
scripts/deploy.sh --env dev              # build, migrate, upload, register, promote, deploy
scripts/deploy.sh --env dev --dry-run    # build and validate the three Workers, change nothing
scripts/deploy.sh --env live --no-build  # reuse the last build
```

The release builds are heavy: don't run them next to `dx` or another build (see `CLAUDE.md`).

### First deploy of an environment

1. Finish the [one-time setup](#one-time-setup) for the environment, including its GitHub
   environment.
2. Create the Workers with their secrets ([above](#workers-and-their-secrets)), or run the
   first deploy with **secrets** checked: it creates a missing services worker with a plain
   `wrangler deploy` before its first `versions upload` (which needs an existing Worker).
3. Run the deploy. If it stops at "no preview URL for the services worker", turn on **Preview
   URLs** (and workers.dev) for `techdepo-<env>-services` in the dashboard (Settings → Domains &
   Routes) and run it again.
4. Add the environment's Email Routing rule.
5. Check it. Open the shop and sign in: the first request provisions your org. In the Restate
   Cloud UI, the deployment shows every service. Send a mail to the support address shown under
   Account. For live, the custom domain `techdepo.dev` is created by the web worker's deploy
   (`routes` in `apps/web-worker/wrangler.toml`).
6. Turn on the [Muhaha page](#the-operators-page-muhaha) for your org.

### Rolling back

Restate sends new invocations to the most recently registered deployment, so a rollback is a new
deploy of the older code: revert on `main` (live), or run the workflow on an older ref (dev).
`wrangler versions deploy` alone would move HTTP traffic but not Restate's.

### Old Restate deployments

Each deploy adds a deployment to the Restate environment. Old ones drain as their invocations
finish. Remove drained ones now and then: in the Restate Cloud UI, or with
`DELETE $RESTATE_ADMIN_URL/deployments/<id>` (`GET /deployments` lists them). Their Worker
versions stay reachable on their preview URLs until then, so never delete a Worker version that a
live deployment still points to.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Registration fails with a 5xx from the preview URL | The services worker refused to start: `RESTATE_IDENTITY_KEYS` is missing, or `RESEND_API_KEY` is missing with `EMAIL_TRANSPORT=resend`. Put the secrets and run again. |
| Registration fails with 401/403 | `RESTATE_ADMIN_TOKEN` lacks the admin role, or belongs to the other environment. |
| Every mutation answers 503 `unavailable` | The web worker cannot reach ingress: check `RESTATE_INGRESS_URL` and `RESTATE_API_KEY` (ingress role). |
| Sign-in loops or `/api/*` answers 401 | Clerk keys from the wrong instance, or the shop URL is missing from `CLERK_AUTHORIZED_PARTIES`. |
| Clerk says "The External Account was not found" on a social sign-in | A new social account (no user in this Clerk instance yet; local, dev and live each have their own users) whose sign-in Clerk could not turn into a sign-up. Either sign-ups are blocked (Configure → Restrictions → **Sign-up mode** must be Public), or the SPA lost the widget's `#/sso-callback` on the way back from the provider: the `SignIn` route must keep its `#:clerk` hash field (`apps/web/src/routes.rs`). Existing users are not affected. |
| A model provider is "not configured" on the Muhaha page | Its API key is not a secret of `techdepo-<env>-services`. Put it (or deploy with **secrets** checked and the key in the GitHub environment); the next request picks it up. |
| Every agent reply escalates with "… is not configured in this deployment" | The saved model settings name a provider whose key was removed since. Pick another provider on the Muhaha page, or put the key back. |
| No Muhaha entry in the admin sidebar | The org lacks the `muhaha` feature ([above](#the-operators-page-muhaha)), or the user is not an admin of it. |
| Support mail never arrives | The Email Routing rule is missing, or points to the other environment's mail-in. Mail-in only accepts mail with `dmarc=pass`. |
