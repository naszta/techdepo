//! The public landing page that explains the demo (roadmap M7, commerce.md §4): what TechDepó
//! is (and is not), which parts are code, a durable workflow, an AI agent or a person, and how
//! to drive it: sign in, demo controls, scenario presets, support email, admin.
//!
//! Linked from the permanent demo banner, so it is one click away on every page.

use dioxus::prelude::*;
use techdepo_api_types::session::AuthMode;

use crate::auth::{SignedIn, SignedOut};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::{Icon, IconName, Mono};

#[component]
pub fn DemoGuide() -> Element {
    rsx! {
        div { class: "flex flex-col gap-12 py-4 sm:py-8",
            Intro {}
            NotAShop {}
            FourAnswers {}
            HowToTry {}
            ThingsToTry {}
        }
    }
}

#[component]
fn Intro() -> Element {
    rsx! {
        section { class: "flex flex-col items-start gap-5",
            span { class: "badge badge-soft badge-warning gap-1",
                Icon { name: IconName::Flask, class: "size-3.5" }
                "Demo · not a real shop"
            }
            h1 { class: "max-w-3xl text-4xl font-semibold tracking-tight sm:text-5xl",
                "A PC-parts shop that shows its work."
            }
            p { class: "max-w-2xl text-lg text-base-content/70",
                "TechDepó looks and behaves like a real webshop, but it is a demo. It shows "
                strong { class: "text-base-content", "what should be code, what should be a durable workflow, what should be an AI agent, and when a person must decide" }
                ", in an order lifecycle and a support inbox you can break on purpose."
            }
            div { class: "flex flex-wrap gap-3",
                SignedOut {
                    Link { to: Route::sign_in_to("/"), class: "btn btn-primary",
                        "Sign in to try it"
                        Icon { name: IconName::ArrowRight }
                    }
                }
                SignedIn {
                    Link { to: Route::AccountOrders {}, class: "btn btn-primary",
                        Icon { name: IconName::Package }
                        "My orders"
                    }
                }
                Link { to: Route::Home {}, class: "btn btn-outline",
                    Icon { name: IconName::Store }
                    "Look around the shop"
                }
            }
        }
    }
}

/// The phishing question, answered before anything else.
#[component]
fn NotAShop() -> Element {
    rsx! {
        section { class: "rounded-box border border-base-300 bg-base-200/60 p-5 sm:p-6",
            h2 { class: "mb-4 flex items-center gap-2 text-lg font-semibold",
                Icon { name: IconName::ShieldCheck, class: "size-5 text-primary" }
                "Nothing here is real"
            }
            ul { class: "grid gap-3 text-sm text-base-content/80 sm:grid-cols-2",
                Fact { "Brands and products are fictional, built on real standards (sockets, memory, power)." }
                Fact { "Nothing ships and nothing is charged. Checkout never asks for card details: payments are simulated." }
                Fact { "Each shop is an isolated sandbox. Orders, tickets and stock are only visible to its members." }
                Fact { "Support only answers members of the shop, and its emails only go to them." }
            }
        }
    }
}

#[component]
fn Fact(children: Element) -> Element {
    rsx! {
        li { class: "flex gap-2",
            Icon { name: IconName::Check, class: "mt-0.5 size-4 shrink-0 text-success" }
            span { {children} }
        }
    }
}

/// Code, workflow, agent, person: the point of the demo.
#[component]
fn FourAnswers() -> Element {
    rsx! {
        section { class: "flex flex-col gap-4", aria_labelledby: "four-answers",
            div { class: "flex flex-col gap-1",
                h2 { id: "four-answers", class: "text-2xl font-semibold tracking-tight", "Four answers to “who handles this?”" }
                p { class: "text-base-content/70",
                    "Every step in the shop is one of these, on purpose. The admin area shows which one ran and why."
                }
            }
            div { class: "grid gap-4 sm:grid-cols-2 lg:grid-cols-4",
                Answer {
                    icon: IconName::Braces,
                    title: "Code",
                    when: "When the rule is known.",
                    examples: vec![
                        "The compatibility checker (sockets, clearances, power)",
                        "Shipping policy: 3 workdays is normal, 5 opens an investigation",
                        "The duplicate-charge detector",
                        "Invoice numbering and VAT",
                    ],
                }
                Answer {
                    icon: IconName::Route,
                    title: "Durable workflow",
                    when: "When it takes time, can fail and must finish.",
                    examples: vec![
                        "The order lifecycle, from payment to delivery",
                        "Refunds and lost-parcel investigations",
                        "Emails retried through a provider outage",
                        "Waiting days for a person or a customer reply",
                    ],
                }
                Answer {
                    icon: IconName::Bot,
                    title: "AI agent",
                    when: "When the steps can't be known up front.",
                    examples: vec![
                        "Reading every support email and scoring its intent",
                        "Product advice, checked against the compatibility engine",
                        "Open-ended order questions, with read-only tools",
                        "Understanding a reply like “just send a new one”",
                    ],
                }
                Answer {
                    icon: IconName::Hand,
                    title: "A person",
                    when: "When money moves or confidence is low.",
                    examples: vec![
                        "Approving a refund",
                        "Choosing refund or reship for a lost parcel",
                        "Vague, angry or legal emails",
                        "Reviewing AI replies, if the reply policy says so",
                    ],
                }
            }
            p { class: "text-sm text-base-content/60",
                "Built in Rust on Cloudflare Workers, with Restate for durable execution and Workers AI for classification and agents."
            }
        }
    }
}

#[component]
fn Answer(
    icon: IconName,
    title: &'static str,
    when: &'static str,
    examples: Vec<&'static str>,
) -> Element {
    rsx! {
        div { class: "card card-border border-base-300 bg-base-100",
            div { class: "card-body gap-3 p-5",
                div { class: "grid size-10 place-items-center rounded-box bg-primary/8 text-primary",
                    Icon { name: icon, class: "size-5" }
                }
                div { class: "flex flex-col gap-0.5",
                    h3 { class: "font-semibold", "{title}" }
                    p { class: "text-sm text-base-content/70", "{when}" }
                }
                ul { class: "list-disc space-y-1 pl-4 text-sm text-base-content/80",
                    for example in examples {
                        li { "{example}" }
                    }
                }
            }
        }
    }
}

/// The demo's moves, in the order a first visit makes them.
#[component]
fn HowToTry() -> Element {
    let session = use_session();
    let clerk = session.auth_mode() == Some(AuthMode::Clerk);
    rsx! {
        section { class: "flex flex-col gap-4", aria_labelledby: "how-to-try",
            h2 { id: "how-to-try", class: "text-2xl font-semibold tracking-tight", "How to try it" }
            ol { class: "grid gap-4 md:grid-cols-2",
                Step { number: 1, icon: IconName::User, title: "Sign in and get your own shop",
                    if clerk {
                        "Create or pick an organization when you sign in: each one is its own shop, with its own stock, orders and support address. You are its customer and its admin."
                    } else {
                        "Any shop name and email will do (local dev login). Each shop name is its own shop, with its own stock, orders and support address. You are its customer and its admin."
                    }
                }
                Step { number: 2, icon: IconName::Wrench, title: "Build a PC and order it",
                    "The builder checks every part against the rest and says why something doesn't fit. Checkout asks for an address and a payment method; no card details, the payment is simulated."
                }
                Step { number: 3, icon: IconName::Flask, title: "Play the outside world",
                    "The amber "
                    strong { "Demo controls" }
                    " button on an order moves time forward, pays, ships and delivers, and injects faults: charge twice, an invoice without an address, a lost parcel, an email outage. "
                    strong { "Scenario presets" }
                    " skip straight to an order in an interesting state."
                }
                Step { number: 4, icon: IconName::Mail, title: "Email support",
                    "Write from the address you signed in with to "
                    SupportAddress {}
                    ", or use “Send as email” on the support page. Simple requests are answered at once; anything that moves money waits for a person."
                }
                Step { number: 5, icon: IconName::Dashboard, title: "Watch it happen in admin",
                    "Human tasks wait for your decision. Tickets show the classification probabilities, the route taken and every agent step; orders show their full event timeline."
                }
                Step { number: 6, icon: IconName::Sliders, title: "Turn the dials",
                    "Settings hold the escalation threshold and the AI reply policy (always send, always review, or send above a confidence), and reset the whole shop when you are done."
                }
            }
        }
    }
}

/// The caller's support address when signed in, its shape otherwise.
#[component]
fn SupportAddress() -> Element {
    let session = use_session();
    if let Some(me) = session.me() {
        let address = me.org.support_address;
        return rsx! {
            a { class: "link link-primary break-all", href: "mailto:{address}", Mono { "{address}" } }
        };
    }
    let domain = session
        .config
        .read()
        .as_ref()
        .map(|c| c.support_domain.clone())
        .unwrap_or_default();
    rsx! {
        Mono { "support+<shop ID>@{domain}" }
        " (your account page shows it)"
    }
}

#[component]
fn Step(number: u8, icon: IconName, title: &'static str, children: Element) -> Element {
    rsx! {
        li { class: "flex gap-4 rounded-box border border-base-300 bg-base-100 p-5",
            div { class: "grid size-8 shrink-0 place-items-center rounded-full bg-primary text-sm font-semibold text-primary-content",
                "{number}"
            }
            div { class: "flex min-w-0 flex-col gap-1",
                h3 { class: "flex items-center gap-2 font-semibold",
                    Icon { name: icon, class: "size-4 text-base-content/60" }
                    "{title}"
                }
                p { class: "text-sm text-base-content/75", {children} }
            }
        }
    }
}

/// A few of the demo scenarios (roadmap.md), as things to say to support.
#[component]
fn ThingsToTry() -> Element {
    rsx! {
        section { class: "flex flex-col gap-4", aria_labelledby: "things-to-try",
            div { class: "flex flex-col gap-1",
                h2 { id: "things-to-try", class: "text-2xl font-semibold tracking-tight", "Things to try" }
                p { class: "text-base-content/70", "Start from a preset, then email support and see who answers." }
            }
            // A table from `md` on; stacked, labelled rows on phones.
            div { class: "rounded-box border border-base-300 bg-base-100 text-sm",
                div { class: "hidden border-b border-base-300 px-4 py-2 text-xs font-semibold text-base-content/60 md:grid md:grid-cols-[11rem_16rem_1fr] md:gap-4",
                    span { "Start from" }
                    span { "Then say" }
                    span { "What happens" }
                }
                ul { class: "divide-y divide-base-300",
                        Scenario {
                            start: "Double-charged",
                            say: "“I was charged twice.”",
                            outcome: "Code finds the duplicate, a refund workflow starts and waits for your approval.",
                        }
                        Scenario {
                            start: "Invoice without address",
                            say: "“My invoice has no address.”",
                            outcome: "The invoice is voided and reissued automatically: safe, so no person needed.",
                        }
                        Scenario {
                            start: "Shipped last Friday",
                            say: "“Where is my order?”",
                            outcome: "Policy as code: “on its way” up to 3 workdays, “please wait” on day 4, an investigation from day 5. Move the clock to see each.",
                        }
                        Scenario {
                            start: "Lost, gone",
                            say: "“Where is my order?” after 5 workdays",
                            outcome: "An investigation finds it lost; you offer refund or reship, the customer's reply is read by AI.",
                        }
                        Scenario {
                            start: "Any order",
                            say: "“Will the Kestrel B650 work with my Nimbus DDR4 kit?”",
                            outcome: "An agent answers, grounded in the compatibility engine, with a DDR5 suggestion.",
                        }
                        Scenario {
                            start: "Any order",
                            say: "Something vague or angry",
                            outcome: "Low confidence: escalated to a person with full context.",
                        }
                }
            }
        }
    }
}

#[component]
fn Scenario(start: &'static str, say: &'static str, outcome: &'static str) -> Element {
    rsx! {
        li { class: "grid gap-1 px-4 py-3 md:grid-cols-[11rem_16rem_1fr] md:gap-4",
            span { class: "font-medium",
                span { class: "text-base-content/60 font-normal md:hidden", "Start from: " }
                "{start}"
            }
            span {
                span { class: "text-base-content/60 md:hidden", "Then say: " }
                "{say}"
            }
            span { class: "text-base-content/70", "{outcome}" }
        }
    }
}
