#!/usr/bin/env bash
# Builds the TechDepó SPA (apps/web) and copies the static output into
# apps/web-worker/public, the Static Assets directory of the techdepo-web worker.
#
#   scripts/build-web.sh            # release build (wasm-opt, minified CSS)
#   scripts/build-web.sh --debug    # faster debug build
#   scripts/build-web.sh --serve    # dev: Tailwind --watch + `dx serve` on :8090, /api -> :8787
#
# Steps: Tailwind CSS (npx @tailwindcss/cli) -> dx build --platform web -> copy.
#
# dx needs the wasm-bindgen CLI whose version matches Cargo.lock (pinned to 0.2.126, see
# CLAUDE.md). The devenv's dx is built with `no-downloads` and uses whatever `wasm-bindgen` is on
# PATH, so this script puts a matching one first: $WASM_BINDGEN_DIR, worker-build's cache, or a
# release download into ~/.cache/techdepo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB="$ROOT/apps/web"
OUT="$ROOT/apps/web-worker/public"
PROFILE=release
MODE=build
case "${1:-}" in
  --debug) PROFILE=debug ;;
  --serve) MODE=serve ;;
  "") ;;
  *) echo "usage: $0 [--debug|--serve]" >&2; exit 2 ;;
esac

# 0. A wasm-bindgen CLI matching Cargo.lock.
WB_VERSION="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' "$ROOT/Cargo.lock")"
ensure_wasm_bindgen() {
  local have
  have="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" || true
  [[ "$have" == "$WB_VERSION" ]] && return
  local candidates=(
    "${WASM_BINDGEN_DIR:-}"
    "$HOME/.cache/worker-build/wasm-bindgen-x86_64-unknown-linux-musl-$WB_VERSION"
    "$HOME/.cache/techdepo/wasm-bindgen-$WB_VERSION-x86_64-unknown-linux-musl"
  )
  for dir in "${candidates[@]}"; do
    if [[ -n "$dir" && -x "$dir/wasm-bindgen" ]]; then
      export PATH="$dir:$PATH"
      return
    fi
  done
  local name="wasm-bindgen-$WB_VERSION-x86_64-unknown-linux-musl"
  mkdir -p "$HOME/.cache/techdepo"
  echo "build-web: downloading $name" >&2
  curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$WB_VERSION/$name.tar.gz" \
    | tar -xz -C "$HOME/.cache/techdepo"
  export PATH="$HOME/.cache/techdepo/$name:$PATH"
}
ensure_wasm_bindgen
echo "build-web: using $(wasm-bindgen --version)"

cd "$WEB"

# 1. CSS. node_modules is git-ignored; install it once.
if [[ ! -d node_modules ]]; then
  npm ci --no-audit --no-fund
fi
npx tailwindcss -i ./tailwind.css -o ./public/tailwind.css --minify

if [[ "$MODE" == serve ]]; then
  # Rebuild the CSS as Rust sources change; dx reloads when public/ changes.
  npx tailwindcss -i ./tailwind.css -o ./public/tailwind.css --watch=always &
  CSS_PID=$!
  trap 'kill "$CSS_PID" 2>/dev/null || true' EXIT
  # Port 8080 belongs to Restate ingress. Dioxus.toml proxies /api to the web worker on 8787.
  # Without a terminal (scripts/dev.sh runs this in the background) dx must not draw its TUI
  # or try to open a browser.
  DX_ARGS=()
  if [[ ! -t 1 ]]; then
    DX_ARGS=(--interactive false --open false)
  fi
  dx serve -p techdepo-web --platform web --port 8090 "${DX_ARGS[@]}"
  exit 0
fi

BUNDLE="$ROOT/target/dx/techdepo-web/$PROFILE/web/public"
# dx keeps old hashed assets around in its output; start from an empty bundle.
rm -rf "$BUNDLE"

# 2. WASM + JS + index.html. dx copies public/ (fonts, CSS) into the bundle root.
# `-p` is required: dx 0.7.10 resolves the workspace's default-members relative to the current
# directory and panics (workspace.rs:325) when none is given.
if [[ "$PROFILE" == release ]]; then
  # Uses [profile.wasm-release] from the root Cargo.toml (strips debuginfo so wasm-opt runs).
  dx build -p techdepo-web --platform web --release
else
  dx build -p techdepo-web --platform web
fi

if [[ ! -f "$BUNDLE/index.html" ]]; then
  echo "build-web: expected the bundle at $BUNDLE" >&2
  exit 1
fi

# 3. Replace the worker's static assets (keep the .gitkeep that holds the directory).
find "$OUT" -mindepth 1 ! -name .gitkeep -exec rm -rf {} +
cp -R "$BUNDLE"/. "$OUT"/
echo "build-web: $(find "$OUT" -type f ! -name .gitkeep | wc -l) files in ${OUT#"$ROOT"/}"
# `wrangler dev` reads the asset manifest once at startup and does not pick up new hashed files.
if pgrep -f 'wrangler dev --port 8787' >/dev/null 2>&1; then
  echo "build-web: restart the web worker on :8787 to serve this build (scripts/dev.sh --restart web)" >&2
fi
