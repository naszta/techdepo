# techdepo-web (SPA)

Dioxus 0.7 client-side app on Tailwind CSS v4 + daisyUI 5, served as Static Assets by the
`techdepo-web` worker (`apps/web-worker`). Spec: `docs/plan/design.md` and `docs/plan/api.md`.

## Build and run

```bash
npm ci                                  # once, in apps/web (Tailwind CLI, daisyUI, Geist fonts)
scripts/build-web.sh                    # release build -> apps/web-worker/public
scripts/build-web.sh --serve            # dev: dx serve on :8090, /api proxied to :8787
cargo check -p techdepo-web --target wasm32-unknown-unknown
```

- **CSS** is built by the Tailwind CLI (`npm run css`, or `css:watch`) from `tailwind.css` into
  `public/tailwind.css` (git-ignored) and linked from `index.html`. dx's built-in Tailwind and
  `asset!()` are not used, so `cargo check` never needs a CSS build.
- **Fonts** (Geist, Geist Mono; latin + latin-ext) are self-hosted in `public/fonts`, copied from
  `@fontsource-variable/*`.
- **dx** must be run with `-p techdepo-web` in this workspace, with a `wasm-bindgen` 0.2.126 on
  `PATH` (the devenv's dx never downloads one); `scripts/build-web.sh` takes care of both.
- The theme is applied before the WASM loads by the inline script in `index.html`.

## Layout

| Path | What |
|---|---|
| `src/app.rs` | Root: providers, `/api/config` bootstrap, `ClerkProvider` (clerk mode), router |
| `src/routes.rs` | Every route; shop, `/account/*`, `/admin/*` |
| `src/api.rs` | Typed client over `techdepo_api_types::Endpoint` (gloo-net) |
| `src/session.rs` | Config, auth state, bearer token (Clerk `get_token()` or dev token in localStorage) |
| `src/auth.rs` | `RequireAuth`, `SignedIn`/`SignedOut` guards, header user control |
| `src/state/` | Cart, stock, and the builder's current build (local draft + compat in-browser) |
| `src/forms.rs` | dioform glue: API field errors <-> form field identities |
| `src/components/` | Installed registry components (see its README) |
| `src/ui/` | App-level components: status badges, money, theme toggle, product tiles, icons |
| `src/layouts/` | Shop navbar layout; account/admin sidebar layouts |
| `src/pages/` | One module per page; `account/` and `admin/` hold stubs for later milestones |
