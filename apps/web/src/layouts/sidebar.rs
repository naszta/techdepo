//! Account and admin: a daisyUI `drawer` that is a sidebar on desktop (`lg:drawer-open`) and
//! slides in on mobile, plus a slim top bar with the Shop/Admin switch.
//!
//! The drawer is the CSS-only daisyUI pattern (a checkbox toggle), because the sidebar must stay
//! rendered and visible on desktop while the registry's `Drawer` is a modal dialog.

use dioxus::prelude::*;
use techdepo_api_types::OrgFeature;

use crate::auth::{RequireAuth, UserControl};
use crate::layouts::{Logo, RouteTitle, SystemNotices};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::{DemoBanner, Icon, IconName, ThemeToggle};

/// One sidebar entry.
#[derive(Clone, PartialEq)]
struct NavItem {
    label: &'static str,
    icon: IconName,
    to: Route,
}

fn item(label: &'static str, icon: IconName, to: Route) -> NavItem {
    NavItem { label, icon, to }
}

#[component]
pub fn AccountLayout() -> Element {
    let items = vec![
        item("Overview", IconName::User, Route::AccountHome {}),
        item("Orders", IconName::Package, Route::AccountOrders {}),
        item("Saved builds", IconName::Wrench, Route::AccountBuilds {}),
        item("Support", IconName::LifeBuoy, Route::support()),
        item("Profile", IconName::MapPin, Route::AccountProfile {}),
    ];
    rsx! {
        SidebarFrame { id: "account-drawer", title: "My account", items, admin: false,
            RequireAuth { Outlet::<Route> {} }
        }
    }
}

#[component]
pub fn AdminLayout() -> Element {
    let session = use_session();
    // The operator's tools act across every org: their own section, apart from the shop's admin.
    let operator = if session.has_feature(OrgFeature::Muhaha) {
        vec![item("Muhaha", IconName::Sparkles, Route::AdminMuhaha {})]
    } else {
        Vec::new()
    };
    let items = vec![
        item("Overview", IconName::Dashboard, Route::AdminOverview {}),
        item("Human tasks", IconName::ListChecks, Route::AdminTasks {}),
        item("Orders", IconName::Package, Route::AdminOrders {}),
        item("Tickets", IconName::Ticket, Route::AdminTickets {}),
        item(
            "Investigations",
            IconName::Search,
            Route::AdminInvestigations {},
        ),
        item("Refunds", IconName::Scale, Route::AdminRefunds {}),
        item("Invoices", IconName::Receipt, Route::AdminInvoices {}),
        item("Payments", IconName::CreditCard, Route::AdminPayments {}),
        item("Inventory", IconName::Boxes, Route::AdminInventory {}),
        item("Customers", IconName::Users, Route::AdminCustomers {}),
        item("Email log", IconName::Mail, Route::AdminEmails {}),
        item("Settings", IconName::Sliders, Route::AdminSettings {}),
    ];
    rsx! {
        SidebarFrame { id: "admin-drawer", title: "Admin", items, operator, admin: true,
            div { class: "text-sm",
                RequireAuth { admin: true, Outlet::<Route> {} }
            }
        }
    }
}

#[component]
fn SidebarFrame(
    id: &'static str,
    title: &'static str,
    items: Vec<NavItem>,
    /// The operator section under the main entries (hidden when empty).
    #[props(default)]
    operator: Vec<NavItem>,
    admin: bool,
    children: Element,
) -> Element {
    let route = use_route::<Route>();
    let session = use_session();
    let org = session.me().map(|me| me.org.name);
    let mut drawer_open = use_signal(|| false);

    rsx! {
        RouteTitle {}
        div { class: "flex min-h-screen flex-col",
            DemoBanner {}
            div { class: "drawer lg:drawer-open flex-1",
                input {
                    id,
                    r#type: "checkbox",
                    class: "drawer-toggle",
                    checked: drawer_open(),
                    onchange: move |e| drawer_open.set(e.checked()),
                }
                // From `lg` (where the demo drawer has no backdrop) the content makes room for
                // an open demo drawer (`ui::demo`) instead of hiding under it; pages lay out by
                // `main`'s width (container queries), not the viewport's.
                div { class: "drawer-content flex min-w-0 flex-col lg:[html[data-demo-open]_&]:pr-96",
                    header { class: "sticky top-0 z-20 border-b border-base-300 bg-base-100/95 backdrop-blur",
                        div { class: "navbar min-h-14 gap-2 px-4",
                            // A real button (focusable, Enter/Space): daisyUI's checkbox is
                            // visually hidden, and a `label` for it is not focusable.
                            button {
                                r#type: "button",
                                class: "btn btn-ghost btn-square lg:hidden",
                                aria_label: "Open navigation",
                                aria_controls: "{id}-side",
                                aria_expanded: "{drawer_open()}",
                                onclick: move |_| drawer_open.set(true),
                                Icon { name: IconName::Menu, class: "size-5" }
                            }
                            div { class: "lg:hidden", Logo {} }
                            div { class: "flex-1" }
                            ModeSwitch { admin }
                            ThemeToggle {}
                            UserControl {}
                        }
                    }
                    SystemNotices {}
                    // Admin is denser and wider (split views, ledgers); account stays readable.
                    main {
                        class: "@container mx-auto w-full flex-1 px-4 py-6 sm:px-6",
                        class: if admin { "max-w-7xl" } else { "max-w-6xl" },
                        {children}
                    }
                }
                div { id: "{id}-side", class: "drawer-side z-30 border-r border-base-300",
                    label { r#for: id, class: "drawer-overlay", aria_label: "Close navigation" }
                    aside { class: "flex min-h-full w-64 flex-col gap-4 bg-base-200 p-4",
                        div { class: "px-2 pt-1", Logo {} }
                        div { class: "px-2",
                            p { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "{title}" }
                            if let Some(org) = org {
                                p { class: "truncate text-sm text-base-content/70", "{org}" }
                            }
                        }
                        ul { class: "menu w-full gap-0.5 p-0",
                            for entry in items {
                                li {
                                    NavLink { active: is_active(&route, &entry.to), entry, drawer_open }
                                }
                            }
                        }
                        if !operator.is_empty() {
                            ul { class: "menu w-full gap-0.5 p-0",
                                li { class: "menu-title", "Operator" }
                                for entry in operator {
                                    li {
                                        NavLink { active: is_active(&route, &entry.to), entry, drawer_open }
                                    }
                                }
                            }
                        }
                        div { class: "mt-auto px-2 text-xs text-base-content/50",
                            Link { to: Route::Home {}, class: "link link-hover", "← Back to the shop" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NavLink(entry: NavItem, active: bool, drawer_open: Signal<bool>) -> Element {
    rsx! {
        Link {
            to: entry.to,
            class: if active { "menu-active bg-primary/10 text-primary" } else { "" },
            onclick: move |_| drawer_open.set(false),
            Icon { name: entry.icon }
            "{entry.label}"
        }
    }
}

/// The overview entries match exactly; the others also match their detail pages.
fn is_active(current: &Route, target: &Route) -> bool {
    let path = |route: &Route| {
        let route = route.to_string();
        route.split('?').next().unwrap_or_default().to_owned()
    };
    let (current, target) = (path(current), path(target));
    if target.trim_end_matches('/').matches('/').count() <= 1 {
        current.trim_end_matches('/') == target.trim_end_matches('/')
    } else {
        current == target || current.starts_with(&format!("{target}/"))
    }
}

/// Shop ↔ Admin: one user plays both roles.
#[component]
fn ModeSwitch(admin: bool) -> Element {
    let session = use_session();
    if !session.is_admin() {
        return rsx! {
            Link { to: Route::Home {}, class: "btn btn-ghost btn-sm", aria_label: "Shop",
                Icon { name: IconName::Store }
                span { class: "hidden sm:inline", "Shop" }
            }
        };
    }
    rsx! {
        div { class: "join", role: "group", aria_label: "Switch area",
            Link {
                to: Route::Home {},
                aria_label: "Shop",
                class: if admin { "btn btn-sm join-item" } else { "btn btn-sm join-item btn-active" },
                Icon { name: IconName::Store }
                span { class: "hidden sm:inline", "Shop" }
            }
            Link {
                to: Route::AdminOverview {},
                aria_label: "Admin",
                class: if admin { "btn btn-sm join-item btn-active" } else { "btn btn-sm join-item" },
                Icon { name: IconName::Dashboard }
                span { class: "hidden sm:inline", "Admin" }
            }
        }
    }
}
