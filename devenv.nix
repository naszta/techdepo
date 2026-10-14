{ pkgs, config, ... }:

let
  root = config.devenv.root;
  persist = "${root}/.wrangler/state";

  # A global wrangler when there is one, `npx wrangler@4` otherwise (nodejs is in the shell).
  wrangler = ''
    if command -v wrangler >/dev/null 2>&1; then WRANGLER=(wrangler); else WRANGLER=(npx --yes wrangler@4); fi
  '';

  # `wrangler dev` for one worker. Every worker shares --persist-to, so they see one local D1.
  # worker-build runs in --dev mode unless WORKER_BUILD says otherwise (`WORKER_BUILD=--release
  # devenv up` for optimized builds).
  # `pre` is shell run first; it may set ARGS (extra `wrangler dev` arguments).
  worker = dir: port: path: pre: {
    cwd = "${root}/apps/${dir}";
    exec = ''
      ${wrangler}
      export WORKER_BUILD="''${WORKER_BUILD:---dev}"
      ARGS=()
      ${pre}
      exec "''${WRANGLER[@]}" dev --port ${toString port} --ip 0.0.0.0 --persist-to ${persist} "''${ARGS[@]}"
    '';
    # wrangler opens the port only after the (minutes-long, on a cold cache) worker build; the probe
    # runs until the first 2xx and no deadline applies.
    ready = {
      http.get = { inherit port path; };
      period = 3;
    };
  };
in
{
  dotenv.enable = true;

  packages = with pkgs; [
    # The CI recipes (justfile); .github/workflows run them in this shell.
    just

    lld
    cargo-audit
    cargo-deny
    cargo-dist
    cargo-release
    cargo-watch

    # scripts/: npm/npx for the SPA's Tailwind + daisyUI toolchain (apps/web/package.json) and
    # wrangler (`npx wrangler@4` when it is not installed globally); jq + curl for dev.sh/smoke.sh
    # and the register task below.
    nodejs
    jq
    curl
  ];

  dioxus = {
    enable = true;
  };

  worker-build = {
    enable = true;
  };

  languages = {
    rust = {
      enable = true;
      channel = "stable";
      targets = [ "wasm32-unknown-unknown" ];
    };
  };

  # `devenv up` starts the whole local stack (architecture.md §8), the same parts as scripts/dev.sh
  # but in the foreground: Restate + Mailpit, the three workers, dx serve, and the one-shot tasks
  # around them. `devenv up services web` starts a subset (plus what they need).
  #
  # Memory: the dev machine has ~8 GB and no swap, so builds never overlap. Each worker waits for
  # the previous one to answer (its build is done), and the SPA build and dx serve come after the
  # workers: infra -> services (+ register) -> build-web -> web -> mail-in -> dx.
  processes = {
    # Stopping `devenv up` stops the containers; their data (the restate-data volume) is kept.
    infra = {
      cwd = root;
      exec = "exec docker compose -f deploy/compose.yaml up";
      ready = {
        http.get = {
          port = 9070;
          path = "/health";
        };
        period = 2;
      };
      shutdown.grace = 30;
    };

    # The Restate endpoint's own health route (`GET /` makes the worker panic).
    # LLM_BACKEND=workers-ai (.env, so also .dev.vars) needs the remote AI binding of
    # `[env.workers-ai]` in apps/services/wrangler.toml, and CLOUDFLARE_API_TOKEN/ACCOUNT_ID (.env).
    services = worker "services" 8788 "/health" ''
      if grep -qx 'LLM_BACKEND="workers-ai"' .dev.vars 2>/dev/null; then ARGS=(--env workers-ai); fi
    '';

    web = worker "web-worker" 8787 "/api/config" "" // {
      after = [ "devenv:processes:services" ];
    };

    mail-in = worker "mail-in" 8789 "/" "" // {
      after = [ "devenv:processes:web" ];
    };

    # SPA with hot reload on :8090, /api proxied to the web worker (Dioxus.toml).
    dx = {
      cwd = root;
      exec = "exec scripts/build-web.sh --serve";
      after = [ "devenv:processes:mail-in" ];
    };
  };

  tasks = {
    "techdepo:dev-vars" = {
      description = "Write apps/*/.dev.vars from .env";
      cwd = root;
      exec = "cargo run -q -p xtask -- dev-vars";
      before = [
        "devenv:processes:services"
        "devenv:processes:web"
        "devenv:processes:mail-in"
      ];
    };

    "techdepo:migrate" = {
      description = "Apply the D1 migrations to the local database";
      cwd = "${root}/apps/services";
      exec = ''
        ${wrangler}
        CI=1 "''${WRANGLER[@]}" d1 migrations apply techdepo --local --persist-to ${persist}
      '';
      before = [
        "devenv:processes:services"
        "devenv:processes:web"
      ];
    };

    # The SPA the web worker serves, built when there is none yet (scripts/build-web.sh rebuilds
    # it; restart the web process afterwards, wrangler dev reads the asset manifest once). After
    # the services worker so the two builds do not overlap.
    "techdepo:build-web" = {
      description = "Build the SPA into apps/web-worker/public if it is missing";
      cwd = root;
      status = "test -f apps/web-worker/public/index.html";
      exec = "scripts/build-web.sh";
      after = [ "devenv:processes:services" ];
      before = [ "devenv:processes:web" ];
    };

    # Downstream of the services process, so it needs wantedBy to run under `devenv up` (whose
    # default mode only pulls in upstream tasks).
    "techdepo:register" = {
      description = "Register the services worker with Restate";
      exec = ''
        curl -fsS -X POST "''${RESTATE_ADMIN_URL:-http://localhost:9070}/deployments" \
          -H 'content-type: application/json' \
          -d '{"uri":"http://host.docker.internal:8788","use_http_11":true,"force":true}' >/dev/null
        echo "registered :8788 with Restate"
      '';
      after = [
        "devenv:processes:infra"
        "devenv:processes:services"
      ];
      wantedBy = [ "devenv:processes:services" ];
    };
  };
}
