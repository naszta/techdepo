#!/usr/bin/env bash
# Starts the whole local TechDepó stack in the background (`devenv up` starts the same stack in the
# foreground, see devenv.nix; keep the two in step):
#
#   1. Restate + Mailpit (docker compose, deploy/compose.yaml)
#   2. apps/*/.dev.vars from .env (cargo run -p xtask -- dev-vars)
#   3. local D1 migrations (.wrangler/state, shared by every worker)
#   4. the SPA build in apps/web-worker/public, if there is none yet (or --build-web)
#   5. techdepo-services on :8788, registered with Restate
#   6. techdepo-web on :8787 (API + SPA), techdepo-mail-in on :8789 (when it has a wrangler.toml)
#   7. dx serve on :8090 (SPA with hot reload, /api proxied to :8787)
#
#   scripts/dev.sh                  # start everything that is not running yet
#   scripts/dev.sh --no-dx          # skip dx serve (use the SPA build on :8787)
#   scripts/dev.sh --build-web      # rebuild the SPA (release) before starting the web worker
#   scripts/dev.sh --release        # optimized worker builds (slow); default is worker-build --dev
#   scripts/dev.sh --restart web    # restart one part: services | web | mail-in | dx
#   scripts/dev-down.sh             # stop it all again
#
# Needs: docker, cargo (+ wasm32-unknown-unknown, worker-build, dx), node/npm (Tailwind, and
# wrangler via `npx wrangler@4` unless installed), curl; scripts/smoke.sh also uses jq. The devenv
# shell (devenv.nix) provides all of them except docker.
#
# Logs: .wrangler/dev/logs/<name>.log. Every part is skipped when its port is already taken, so
# running this twice is harmless. `wrangler dev` does not pick up a rebuilt SPA: after
# scripts/build-web.sh, run `scripts/dev.sh --restart web`.
#
# Memory: the dev machine has ~8 GB and no swap. Builds run one after another (and cargo is capped
# at 2 jobs in .cargo/config.toml), but dx serve recompiles the SPA on every save next to two
# workerd processes: prefer --no-dx there, and never start a second cargo/dx build by hand while
# this script is still building.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE="$ROOT/.wrangler/dev"
LOGS="$STATE/logs"
PIDS="$STATE/pids"
PERSIST="$ROOT/.wrangler/state"
mkdir -p "$LOGS" "$PIDS"

DX=1
BUILD_WEB=0
RESTART=""
export WORKER_BUILD="${WORKER_BUILD:---dev}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-dx) DX=0 ;;
    --build-web) BUILD_WEB=1 ;;
    --release) WORKER_BUILD=--release ;;
    --restart)
      RESTART="${2:-}"
      shift
      [[ "$RESTART" =~ ^(services|web|mail-in|dx)$ ]] || {
        echo "usage: $0 --restart services|web|mail-in|dx" >&2
        exit 2
      }
      ;;
    -h | --help)
      awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
      exit 0
      ;;
    *)
      echo "unknown option $1 (see $0 --help)" >&2
      exit 2
      ;;
  esac
  shift
done

RESTATE_ADMIN="${RESTATE_ADMIN_URL:-http://localhost:9070}"

say() { printf '\033[1mdev:\033[0m %s\n' "$*"; }
die() {
  printf '\033[31mdev: %s\033[0m\n' "$*" >&2
  exit 1
}

if command -v wrangler >/dev/null 2>&1; then
  WRANGLER=(wrangler)
elif command -v npx >/dev/null 2>&1; then
  WRANGLER=(npx --yes wrangler@4)
else
  die "wrangler not found (npm i -g wrangler, or enter the devenv shell for nodejs)"
fi
command -v docker >/dev/null 2>&1 || die "docker is required for Restate and Mailpit"
command -v cargo >/dev/null 2>&1 || die "cargo is required"

port_busy() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

# wait_port PORT SECONDS NAME: until something accepts connections on PORT.
wait_port() {
  local port=$1 deadline=$((SECONDS + $2)) name=$3
  until port_busy "$port"; do
    if ((SECONDS > deadline)); then
      die "$name did not come up on :$port (see ${LOGS#"$ROOT"/}/$name.log)"
    fi
    if [[ -f "$PIDS/$name.pid" ]] && ! kill -0 "$(cat "$PIDS/$name.pid")" 2>/dev/null; then
      tail -20 "$LOGS/$name.log" >&2 || true
      die "$name exited (see ${LOGS#"$ROOT"/}/$name.log)"
    fi
    sleep 2
  done
}

# start NAME DIR CMD...: run CMD in DIR in its own session (so dev-down can stop the whole tree).
# The background subshell execs into setsid (not a group leader, so setsid does not fork) and
# then into CMD: the recorded PID is CMD's, which leads its own process group, and no shell is
# left holding this script's stdout open.
start() {
  local name=$1 dir=$2
  shift 2
  (cd "$dir" && exec setsid nohup "$@" >"$LOGS/$name.log" 2>&1 </dev/null) &
  echo $! >"$PIDS/$name.pid"
}

# stop NAME: the process group dev.sh started for NAME.
stop() {
  local name=$1 pid
  [[ -f "$PIDS/$name.pid" ]] || return 0
  pid="$(cat "$PIDS/$name.pid")"
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.5
    done
    kill -KILL -- "-$pid" 2>/dev/null || true
  fi
  rm -f "$PIDS/$name.pid"
}

register_services() {
  local deadline=$((SECONDS + 90)) out
  until out="$(curl -fsS -X POST "$RESTATE_ADMIN/deployments" -H 'content-type: application/json' \
    -d '{"uri":"http://host.docker.internal:8788","use_http_11":true,"force":true}' 2>&1)"; do
    ((SECONDS > deadline)) && die "registering the services worker with Restate failed: $out"
    sleep 2
  done
  say "registered :8788 with Restate ($(printf '%s' "$out" | grep -o '"name":"[A-Za-z]*","ty"' | wc -l) services)"
}

# LLM_BACKEND=workers-ai (.env, so also the services' .dev.vars) runs the services worker with
# `--env workers-ai`: the remote AI binding (apps/services/wrangler.toml), which needs
# CLOUDFLARE_API_TOKEN (+ CLOUDFLARE_ACCOUNT_ID) in wrangler's environment, from .env unless set.
services_args() {
  SERVICES_ARGS=()
  grep -qx 'LLM_BACKEND="workers-ai"' "$ROOT/apps/services/.dev.vars" 2>/dev/null || return 0
  SERVICES_ARGS=(--env workers-ai)
  local key value
  for key in CLOUDFLARE_API_TOKEN CLOUDFLARE_ACCOUNT_ID; do
    [[ -n "${!key:-}" ]] && continue
    value="$(sed -n "s/^\(export \)\{0,1\}$key=[\"']\{0,1\}\([^\"']*\)[\"']\{0,1\}\$/\2/p" "$ROOT/.env" 2>/dev/null | tail -1)"
    [[ -n "$value" ]] && export "$key=$value"
  done
  [[ -n "${CLOUDFLARE_API_TOKEN:-}" ]] ||
    die "LLM_BACKEND=workers-ai needs CLOUDFLARE_API_TOKEN (and CLOUDFLARE_ACCOUNT_ID) in .env"
}

start_services() {
  if port_busy 8788; then
    say "services worker already on :8788"
  else
    services_args
    say "starting techdepo-services on :8788 (worker-build $WORKER_BUILD; the first build takes minutes)${SERVICES_ARGS[*]:+ with ${SERVICES_ARGS[*]}}"
    start services "$ROOT/apps/services" "${WRANGLER[@]}" dev --port 8788 --ip 0.0.0.0 --persist-to "$PERSIST" "${SERVICES_ARGS[@]}"
    wait_port 8788 900 services
  fi
  register_services
}

start_web() {
  if port_busy 8787; then
    say "web worker already on :8787"
    return
  fi
  say "starting techdepo-web on :8787"
  start web "$ROOT/apps/web-worker" "${WRANGLER[@]}" dev --port 8787 --ip 0.0.0.0 --persist-to "$PERSIST"
  wait_port 8787 900 web
}

start_mail_in() {
  if [[ ! -f "$ROOT/apps/mail-in/wrangler.toml" ]]; then
    say "apps/mail-in has no wrangler.toml yet; skipping :8789"
    return
  fi
  if port_busy 8789; then
    say "mail-in worker already on :8789"
    return
  fi
  say "starting techdepo-mail-in on :8789"
  start mail-in "$ROOT/apps/mail-in" "${WRANGLER[@]}" dev --port 8789 --ip 0.0.0.0 --persist-to "$PERSIST"
  wait_port 8789 900 mail-in
}

start_dx() {
  if port_busy 8090; then
    say "dx serve already on :8090"
    return
  fi
  say "starting dx serve on :8090 (first build takes a while; log: .wrangler/dev/logs/dx.log)"
  start dx "$ROOT" "$ROOT/scripts/build-web.sh" --serve
}

if [[ -n "$RESTART" ]]; then
  case "$RESTART" in
    services)
      stop services
      start_services
      ;;
    web)
      stop web
      start_web
      ;;
    mail-in)
      stop mail-in
      start_mail_in
      ;;
    dx)
      stop dx
      start_dx
      ;;
  esac
  exit 0
fi

# 1. Infrastructure.
say "docker compose up (Restate 8080/9070, Mailpit 1025/8025)"
docker compose -f "$ROOT/deploy/compose.yaml" up -d >/dev/null
deadline=$((SECONDS + 60))
until curl -fsS "$RESTATE_ADMIN/health" >/dev/null 2>&1; do
  ((SECONDS > deadline)) && die "Restate admin is not answering on $RESTATE_ADMIN"
  sleep 1
done

# 2. Worker configuration.
say "writing apps/*/.dev.vars"
(cd "$ROOT" && cargo run -q -p xtask -- dev-vars)

# 3. Schema.
say "applying D1 migrations to the local database"
(cd "$ROOT/apps/services" && CI=1 "${WRANGLER[@]}" d1 migrations apply techdepo --local --persist-to "$PERSIST" >"$LOGS/migrations.log" 2>&1) ||
  {
    cat "$LOGS/migrations.log" >&2
    die "migrations failed"
  }

# 4. The SPA the web worker serves. Built before the worker starts: wrangler dev reads the asset
#    manifest once.
if [[ "$BUILD_WEB" == 1 || ! -f "$ROOT/apps/web-worker/public/index.html" ]]; then
  if port_busy 8787; then
    say "note: the web worker is already running; restart it after the build (--restart web)"
  fi
  say "building the SPA (scripts/build-web.sh)"
  "$ROOT/scripts/build-web.sh" >"$LOGS/build-web.log" 2>&1 || {
    tail -30 "$LOGS/build-web.log" >&2
    die "SPA build failed"
  }
fi

# 5-7. Workers one after another (4 CPUs: parallel worker-builds only slow each other down), then dx.
start_services
start_web
start_mail_in
if [[ "$DX" == 1 ]]; then
  start_dx
fi

DX_LINE=""
if [[ "$DX" == 1 ]]; then
  DX_LINE=$'\n  SPA (dx)       http://localhost:8090        (hot reload; ready once dx.log says so)'
fi
cat <<EOF

TechDepó is running:
  shop + API     http://localhost:8787        (dev login, AUTH_MODE=dev)${DX_LINE}
  Restate UI     http://localhost:9070
  Mailpit        http://localhost:8025
  logs           ${LOGS#"$ROOT"/}/
Stop with scripts/dev-down.sh; smoke test with scripts/smoke.sh.
EOF
