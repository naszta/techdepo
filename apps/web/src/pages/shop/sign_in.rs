//! Sign-in: Clerk's `SignIn` widget in `AUTH_MODE=clerk`, the dev login form in `AUTH_MODE=dev`.
//! Either way the user lands back on `redirect`.

use dioform::prelude::*;
use dioxus::prelude::*;
use techdepo_api_types::Validate;
use techdepo_api_types::endpoints::DevLogin;
use techdepo_api_types::session::{AuthMode, DevLoginRequest};
use techdepo_api_types::vocab::OrgRole;

use crate::api::use_api;
use crate::components::input::InputField;
use crate::forms::{FormErrors, submit_errors, validation_errors};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::{Icon, IconName};

/// Only same-site paths are followed after sign-in.
fn safe_redirect(redirect: &str) -> String {
    if redirect.starts_with('/') && !redirect.starts_with("//") && !redirect.starts_with("/sign-in")
    {
        redirect.to_owned()
    } else {
        "/".to_owned()
    }
}

#[component]
pub fn SignIn(redirect: String, clerk: String) -> Element {
    // `clerk` is the widget's hash route: clerk-js reads it from the URL itself.
    let _ = clerk;
    let session = use_session();
    let nav = navigator();
    let target = safe_redirect(&redirect);

    let signed_in = session.is_signed_in();
    let to = target.clone();
    use_effect(use_reactive!(|signed_in| {
        if signed_in {
            nav.replace(to.clone());
        }
    }));

    rsx! {
        div { class: "mx-auto flex max-w-md flex-col gap-6 py-8",
            match session.auth_mode() {
                Some(AuthMode::Clerk) => rsx! {
                    div { class: "flex justify-center",
                        dioxus_clerk::prelude::SignIn {
                            routing: dioxus_clerk::prelude::Routing::Hash,
                            fallback_redirect_url: target.clone(),
                            fallback: rsx! { div { class: "skeleton h-96 w-full" } },
                        }
                    }
                },
                _ => rsx! { DevLoginForm { redirect: target.clone() } },
            }
            p { class: "text-center text-sm text-base-content/70",
                "New here? "
                Link { to: Route::DemoGuide {}, class: "link link-primary", "How this demo works" }
            }
        }
    }
}

/// The dev login: any shop name and email; the shop (org) is created on first use.
#[derive(Clone, PartialEq, Form)]
struct DevLoginModel {
    org_name: String,
    email: String,
    name: String,
}

impl DevLoginModel {
    fn request(&self) -> DevLoginRequest {
        let optional = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        DevLoginRequest {
            email: self.email.trim().to_owned(),
            name: optional(&self.name),
            org_name: optional(&self.org_name),
            role: OrgRole::Admin,
        }
    }
}

#[component]
fn DevLoginForm(redirect: String) -> Element {
    let session = use_session();
    let api = use_api();
    let nav = navigator();
    let form = use_form_config(
        FormConfig::new(DevLoginModel {
            org_name: "Dev Shop".to_owned(),
            email: String::new(),
            name: String::new(),
        })
        .form_validator("dev_login")
        .check(|ctx| match ctx.form().request().validate() {
            Ok(()) => Vec::new(),
            Err(error) => validation_errors(&error),
        }),
    );
    let fields = DevLoginModel::fields();
    let form_errors: Vec<String> = form
        .visible_form_validation_errors()
        .iter()
        .map(|e| e.error().clone())
        .collect();
    let submitting = form.is_submitting();

    let submit_form = form.clone();
    let onsubmit = move |e: FormEvent| {
        e.prevent_default();
        let redirect = redirect.clone();
        submit_form.submit_async_managed(move |snapshot| async move {
            let request = snapshot.value().request();
            match api.send::<DevLogin>(&[], &request).await {
                Ok(response) => {
                    session.sign_in_dev(response);
                    nav.replace(redirect);
                    SubmitErrors::none()
                }
                Err(error) => submit_errors(&error),
            }
        });
    };

    rsx! {
        div { class: "card card-border border-base-300 bg-base-100",
            form { class: "card-body gap-4", novalidate: true, onsubmit,
                div { class: "flex flex-col gap-1",
                    h1 { class: "card-title text-xl", "Sign in" }
                    p { class: "text-sm text-base-content/70",
                        "Local dev login: no password. Each shop name is its own demo shop, and you are its admin."
                    }
                }
                div { role: "note", class: "alert alert-warning alert-soft text-sm",
                    Icon { name: IconName::Flask, class: "size-4" }
                    span { "AUTH_MODE=dev. Never enabled in production." }
                }
                FormErrors { errors: form_errors }
                InputField {
                    context: form.field(fields.org_name()),
                    label: "Shop name",
                    description: "Use the same name to come back to the same shop.",
                    class: "w-full",
                }
                InputField {
                    context: form.field(fields.email()),
                    label: "Email",
                    r#type: "email",
                    autocomplete: "email",
                    class: "w-full",
                }
                InputField {
                    context: form.field(fields.name()),
                    label: "Name (optional)",
                    autocomplete: "name",
                    class: "w-full",
                }
                button { r#type: "submit", class: "btn btn-primary", disabled: submitting,
                    if submitting {
                        span { class: "loading loading-spinner loading-sm" }
                    }
                    "Sign in"
                }
                Link { to: Route::Home {}, class: "link link-hover text-center text-sm", "Keep browsing" }
            }
        }
    }
}
