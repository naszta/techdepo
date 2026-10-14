#!/usr/bin/env bash
# Deploys TechDepó to one environment, `dev` (techdepo-dev-*, on workers.dev) or `live`
# (techdepo-live-*, on techdepo.dev). docs/deploy.md has the accounts, the one-time setup and
# every value. `.github/workflows/deploy.yaml` runs it; it works the same from a shell.
#
#   1. Release builds, one at a time: the SPA (scripts/build-web.sh) and the three Workers.
#   2. The D1 migrations, on the environment's remote database (migrations are additive, so the
#      running code keeps working on the new schema).
#   3. The services worker as a new version (`wrangler versions upload`). Its preview URL is
#      registered with the environment's Restate Cloud, and only then is the version promoted to
#      100%: Restate keeps every invocation on the deployment that started it, so each version
#      keeps its own URL. A first deploy creates the Worker with a plain `deploy` beforehand.
#   4. The web worker (API + SPA) and the mail-in worker.
#
#   scripts/deploy.sh --env dev               # everything, to dev
#   scripts/deploy.sh --env live --no-build   # reuse apps/*/build and apps/web-worker/public
#   scripts/deploy.sh --env dev --secrets     # also upload each Worker's secrets
#   scripts/deploy.sh --env dev --dry-run     # build and check the three Workers, change nothing
#
# Configuration comes from the environment, then from `deploy/<env>.env` (git-ignored; template
# deploy/env.example) for keys the environment does not set. The root .env holds local values
# and is not read.
#
#   CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN   Workers Scripts:Edit, D1:Edit
#   D1_DATABASE_ID                                `wrangler d1 create techdepo-<env>`
#   RESTATE_INGRESS_URL                           web + mail-in -> Restate Cloud ingress
#   RESTATE_ADMIN_URL, RESTATE_ADMIN_TOKEN        deployment registration
#   CLERK_PUBLISHABLE_KEY                         served to the SPA by GET /api/config
#   CLERK_AUTHORIZED_PARTIES                      optional, default: the environment's shop URL
#   DEPLOY_MESSAGE                                optional, recorded with each version (CI: commit)
#
# Optional default model settings, passed to the Workers as `--var`s when set (otherwise the
# wrangler.toml and code defaults apply; crates/llm/src/config.rs). The operator overrides them
# per task on the Muhaha page (stored in D1, survives deploys):
#
#   LLM_BACKEND                                   services + web: `workers-ai` or `mock`
#   AI_AGENT_MODEL, AI_CLASSIFIER_MODEL           services: model addresses (`anthropic:…`; a
#                                                 bare ID is Workers AI)
#   AI_FALLBACK_CLASSIFIER                        services: `on` / `off`
#   LLM_DAILY_TOKEN_BUDGET_PER_ORG, LLM_DAILY_TOKEN_BUDGET_GLOBAL   services
#
# With --secrets, the non-empty ones of these are uploaded with each Worker's version (additive:
# secrets left out stay as they are). Without it they must already be set
# (`wrangler secret put <KEY> --env <env>` in each apps/<worker>):
#
#   services   RESTATE_IDENTITY_KEYS, RESEND_API_KEY, CLERK_SECRET_KEY, and the optional model
#              provider keys ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, OPENROUTER_API_KEY,
#              AZURE_OPENAI_API_KEY + AZURE_OPENAI_RESOURCE (each makes its provider selectable on
#              the Muhaha page)
#   web        CLERK_SECRET_KEY, RESTATE_API_KEY
#   mail-in    CLERK_SECRET_KEY, RESTATE_API_KEY
#
# Needs: cargo (+ wasm32-unknown-unknown), worker-build, dx, node/npm (wrangler via
# `npx wrangler@4` unless installed), curl, jq. The devenv shell provides all of them.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APPS=(services web-worker mail-in)
# The database_id committed in the wrangler.toml files: local only (wrangler dev ignores it).
PLACEHOLDER_DATABASE_ID="00000000-0000-0000-0000-000000000000"

DEPLOY_ENV="${DEPLOY_ENV:-}"
BUILD=1
SECRETS=0
DRY_RUN=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --env)
      DEPLOY_ENV="${2:-}"
      shift
      ;;
    --env=*) DEPLOY_ENV="${1#--env=}" ;;
    --no-build) BUILD=0 ;;
    --secrets) SECRETS=1 ;;
    --dry-run) DRY_RUN=1 ;;
    -h | --help)
      awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
      exit 0
      ;;
    *)
      echo "unknown option $1 (--help)" >&2
      exit 2
      ;;
  esac
  shift
done

log() { echo "deploy[$DEPLOY_ENV]: $*" >&2; }
die() {
  log "$*"
  exit 1
}

case "$DEPLOY_ENV" in
  dev) SHOP_URL="https://techdepo-dev-web.vibekoders.workers.dev" ;;
  live) SHOP_URL="https://techdepo.dev" ;;
  *) die "--env must be dev or live (--help)" ;;
esac

# KEY=VALUE lines of deploy/<env>.env, for keys the environment leaves unset (not sourced:
# values may contain spaces).
ENV_FILE="$ROOT/deploy/$DEPLOY_ENV.env"
if [[ -f "$ENV_FILE" ]]; then
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#export }"
    [[ "$line" =~ ^([A-Z_][A-Z0-9_]*)=(.*)$ ]] || continue
    key="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    if [[ -n "${!key:-}" ]]; then continue; fi
    value="${value%\"}"
    value="${value#\"}"
    value="${value%\'}"
    value="${value#\'}"
    export "$key=$value"
  done <"$ENV_FILE"
fi

require() {
  local missing=()
  for key in "$@"; do
    [[ -n "${!key:-}" ]] || missing+=("$key")
  done
  [[ ${#missing[@]} -eq 0 ]] || die "missing ${missing[*]} (docs/deploy.md)"
}

if [[ "$DRY_RUN" == 0 ]]; then
  require CLOUDFLARE_ACCOUNT_ID CLOUDFLARE_API_TOKEN D1_DATABASE_ID RESTATE_INGRESS_URL \
    RESTATE_ADMIN_URL RESTATE_ADMIN_TOKEN CLERK_PUBLISHABLE_KEY
  for key in RESTATE_INGRESS_URL RESTATE_ADMIN_URL; do
    if [[ "${!key}" =~ ^https?://(localhost|127\.0\.0\.1|host\.docker\.internal)([:/]|$) ]]; then
      die "$key is local (${!key}): set the Restate Cloud URLs"
    fi
  done
fi
export CLOUDFLARE_ACCOUNT_ID CLOUDFLARE_API_TOKEN
D1_DATABASE_ID="${D1_DATABASE_ID:-$PLACEHOLDER_DATABASE_ID}"
CLERK_AUTHORIZED_PARTIES="${CLERK_AUTHORIZED_PARTIES:-$SHOP_URL}"
MESSAGE="${DEPLOY_MESSAGE:-$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo manual)}"

if command -v wrangler >/dev/null 2>&1; then WRANGLER=(wrangler); else WRANGLER=(npx --yes wrangler@4); fi
export CI=true # no prompts

TMP="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP"
  for app in "${APPS[@]}"; do rm -f "$ROOT/apps/$app/wrangler.deploy.toml"; done
}
trap cleanup EXIT

# 1. Builds. Sequential on purpose: each is a heavy release build (LTO, wasm-opt). The same
# artifacts deploy to either environment: everything environment-specific is configuration.
if [[ "$BUILD" == 1 ]]; then
  log "building the SPA"
  "$ROOT/scripts/build-web.sh"
  for app in "${APPS[@]}"; do
    log "building $app"
    (cd "$ROOT/apps/$app" && worker-build --release)
  done
fi
[[ -f "$ROOT/apps/web-worker/public/index.html" ]] || die "no SPA build in apps/web-worker/public"
for app in "${APPS[@]}"; do
  [[ -f "$ROOT/apps/$app/build/index.js" ]] || die "no build of $app in apps/$app/build"
done

# Each Worker's wrangler.toml as deployed: without the [build] table (the build is done) and with
# the environment's D1 database. Next to the original, so its relative paths (build/, public/,
# ../../migrations) still hold.
for app in "${APPS[@]}"; do
  awk 'substr($0, 1, 1) == "[" { skip = ($0 == "[build]") } !skip' "$ROOT/apps/$app/wrangler.toml" |
    sed "s/$PLACEHOLDER_DATABASE_ID/$D1_DATABASE_ID/" >"$ROOT/apps/$app/wrangler.deploy.toml"
done

# wrangler for one Worker in this environment: `in_app <app> <wrangler args…>`.
in_app() {
  local app="$1"
  shift
  (cd "$ROOT/apps/$app" && "${WRANGLER[@]}" "$@" --config wrangler.deploy.toml --env "$DEPLOY_ENV")
}

# The --secrets-file argument for a Worker: its non-empty secrets as JSON (none: no argument).
secrets_args() {
  [[ "$SECRETS" == 1 ]] || return 0
  local file="$TMP/$1.secrets.json"
  shift
  local args=()
  for key in "$@"; do
    if [[ -n "${!key:-}" ]]; then args+=(--arg "$key" "${!key}"); fi
  done
  [[ ${#args[@]} -gt 0 ]] || return 0
  (umask 077 && jq -n '$ARGS.named' "${args[@]}" >"$file")
  printf '%s\n' --secrets-file "$file"
}

# `--var KEY:VALUE` for each of the keys that is set and non-empty.
optional_vars() {
  for key in "$@"; do
    if [[ -n "${!key:-}" ]]; then printf '%s\n' --var "$key:${!key}"; fi
  done
}

mapfile -t SERVICES_VARS < <(optional_vars LLM_BACKEND AI_AGENT_MODEL AI_CLASSIFIER_MODEL \
  AI_FALLBACK_CLASSIFIER LLM_DAILY_TOKEN_BUDGET_PER_ORG LLM_DAILY_TOKEN_BUDGET_GLOBAL)
mapfile -t WEB_VARS < <(optional_vars LLM_BACKEND)
WEB_VARS+=(
  --var "RESTATE_INGRESS_URL:${RESTATE_INGRESS_URL:-}"
  --var "CLERK_PUBLISHABLE_KEY:${CLERK_PUBLISHABLE_KEY:-}"
  --var "CLERK_AUTHORIZED_PARTIES:$CLERK_AUTHORIZED_PARTIES"
)
MAIL_IN_VARS=(--var "RESTATE_INGRESS_URL:${RESTATE_INGRESS_URL:-}")

if [[ "$DRY_RUN" == 1 ]]; then
  for app in "${APPS[@]}"; do
    log "checking $app"
    in_app "$app" deploy --dry-run --outdir "$TMP/$app"
  done
  log "dry run: nothing uploaded"
  exit 0
fi

# 2. The schema first.
log "applying the D1 migrations"
in_app services d1 migrations apply DB --remote

# 3. The services: upload a version, register its preview URL with Restate, then promote it.
log "uploading the services worker"
mapfile -t args < <(secrets_args services RESTATE_IDENTITY_KEYS RESEND_API_KEY CLERK_SECRET_KEY \
  ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY OPENROUTER_API_KEY \
  AZURE_OPENAI_API_KEY AZURE_OPENAI_RESOURCE)
# `versions upload` needs an existing Worker: the first deploy of an environment creates it with
# a plain `deploy` (nothing calls it before its version is registered below).
SERVICES_WORKER="techdepo-$DEPLOY_ENV-services"
status="$(curl -sS -o /dev/null -w '%{http_code}' \
  -H "authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/scripts/$SERVICES_WORKER/settings")"
case "$status" in
  200) ;;
  404)
    [[ "$SECRETS" == 1 ]] ||
      die "the services worker $SERVICES_WORKER does not exist yet: deploy with --secrets (it refuses to start without them)"
    log "creating the services worker $SERVICES_WORKER"
    in_app services deploy --message "$MESSAGE" "${SERVICES_VARS[@]}" "${args[@]}"
    ;;
  *) die "could not look up the services worker $SERVICES_WORKER (HTTP $status)" ;;
esac
WRANGLER_OUTPUT_FILE_PATH="$TMP/upload.ndjson" in_app services versions upload \
  --message "$MESSAGE" "${SERVICES_VARS[@]}" "${args[@]}"
VERSION_ID="$(jq -r 'select(.type == "version-upload") | .version_id // empty' "$TMP/upload.ndjson")"
PREVIEW_URL="$(jq -r 'select(.type == "version-upload") | .preview_url // empty' "$TMP/upload.ndjson")"
[[ -n "$VERSION_ID" ]] || die "wrangler reported no version ID for the services worker"
[[ -n "$PREVIEW_URL" ]] || die "wrangler reported no preview URL for the services worker (preview_urls in wrangler.toml)"

log "registering $PREVIEW_URL with Restate"
jq -n --arg uri "$PREVIEW_URL" '{uri: $uri, use_http_11: true}' |
  curl --fail-with-body -sS -X POST "${RESTATE_ADMIN_URL%/}/deployments" \
    -H "authorization: Bearer $RESTATE_ADMIN_TOKEN" \
    -H 'content-type: application/json' \
    --data @-
echo

log "promoting the services worker $VERSION_ID"
in_app services versions deploy "$VERSION_ID@100%" --message "$MESSAGE" --yes

# 4. The web and mail-in Workers call the services through Restate ingress.
log "deploying the web worker"
mapfile -t args < <(secrets_args web-worker CLERK_SECRET_KEY RESTATE_API_KEY)
in_app web-worker deploy --message "$MESSAGE" "${WEB_VARS[@]}" "${args[@]}"

log "deploying the mail-in worker"
mapfile -t args < <(secrets_args mail-in CLERK_SECRET_KEY RESTATE_API_KEY)
in_app mail-in deploy --message "$MESSAGE" "${MAIL_IN_VARS[@]}" "${args[@]}"

log "done: services $VERSION_ID at $PREVIEW_URL, shop at $SHOP_URL"
