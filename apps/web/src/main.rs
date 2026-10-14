//! TechDepó SPA: Dioxus 0.7 (CSR) on Tailwind v4 + daisyUI 5.
//!
//! - [`app`]: root component (config bootstrap, auth provider, stores, router).
//! - [`routes`]: every route; [`layouts`] and [`pages`] render them.
//! - [`api`]: typed `/api/*` client over `techdepo-api-types`; [`session`]: config and auth.
//! - [`state`]: cart, stock and the PC builder's current build.
//! - [`components`]: installed from dioxus-daisyui-components (`dx components add`), owned
//!   here; [`ui`]: app-level components (status vocabulary, money, theme switch, product
//!   presentation).

mod api;
mod app;
mod auth;
mod browser;
// Registry code: not every installed component or variant is used.
#[allow(dead_code, unused_imports)]
mod components;
mod forms;
mod layouts;
mod pages;
mod routes;
mod session;
mod state;
mod ui;

fn main() {
    dioxus::launch(app::App);
}
