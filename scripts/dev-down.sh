#!/usr/bin/env bash
# Stops what scripts/dev.sh started: dx serve, the wrangler dev workers and (unless
# --keep-infra) the Restate and Mailpit containers. Container data (the restate-data volume)
# and the local D1 in .wrangler/state are kept.
#
#   scripts/dev-down.sh                 # stop everything
#   scripts/dev-down.sh --keep-infra    # stop the workers and dx, leave Restate + Mailpit running
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIDS="$ROOT/.wrangler/dev/pids"

KEEP_INFRA=0
case "${1:-}" in
  --keep-infra) KEEP_INFRA=1 ;;
  "") ;;
  -h | --help)
      awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
    exit 0
    ;;
  *)
    echo "usage: $0 [--keep-infra]" >&2
    exit 2
    ;;
esac

say() { printf '\033[1mdev-down:\033[0m %s\n' "$*"; }

# kill_tree PID: PID and all its descendants (for processes started outside dev.sh).
kill_tree() {
  local pid=$1 child
  for child in $(pgrep -P "$pid" 2>/dev/null); do
    kill_tree "$child"
  done
  kill -TERM "$pid" 2>/dev/null || true
}

# Processes dev.sh started: each runs in its own session, so stop the whole process group.
for file in "$PIDS"/*.pid; do
  [[ -e "$file" ]] || continue
  name="$(basename "$file" .pid)"
  pid="$(cat "$file")"
  if kill -0 "$pid" 2>/dev/null; then
    say "stopping $name"
    kill -TERM -- "-$pid" 2>/dev/null || kill_tree "$pid"
    for _ in $(seq 1 20); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.5
    done
    kill -KILL -- "-$pid" 2>/dev/null || true
  fi
  rm -f "$file"
done

# Workers or dx started by hand with the documented commands.
for pattern in 'wrangler dev --port 878[789]' 'dx serve -p techdepo-web'; do
  for pid in $(pgrep -f "$pattern" 2>/dev/null); do
    # Not a shell whose command line merely mentions the pattern (e.g. `bash -c "... wrangler dev"`).
    case "$(ps -o comm= -p "$pid" 2>/dev/null)" in bash | sh | zsh | dash | "") continue ;; esac
    say "stopping pid $pid ($(ps -o args= -p "$pid" | cut -c1-60))"
    kill_tree "$pid"
  done
done
pkill -f 'tailwindcss -i ./tailwind.css -o ./public/tailwind.css --watch' 2>/dev/null || true

if [[ "$KEEP_INFRA" == 0 ]]; then
  say "docker compose stop (Restate, Mailpit)"
  docker compose -f "$ROOT/deploy/compose.yaml" stop >/dev/null
fi
say "done"
