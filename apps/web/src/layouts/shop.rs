//! The shop frame: demo banner, top navbar, centred content (max ~1200 px), footer.

use dioxus::prelude::*;
use techdepo_api_types::Category;

use crate::auth::{SignedIn, UserControl};
use crate::browser;
use crate::components::dropdown_menu::{
    DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
};
use crate::layouts::search::SearchBox;
use crate::layouts::{Logo, RouteTitle, SystemNotices};
use crate::routes::Route;
use crate::session::use_session;
use crate::state::use_cart;
use crate::ui::product::{category_icon, category_plural};
use crate::ui::{DemoBanner, Icon, IconName, ThemeToggle};

#[component]
pub fn ShopLayout() -> Element {
    // The router keeps the scroll position across pages; start each page at the top.
    let route = use_route::<Route>();
    use_effect(use_reactive!(|route| {
        let _ = &route;
        browser::scroll_to_top();
    }));
    rsx! {
        RouteTitle {}
        div { class: "flex min-h-screen flex-col",
            DemoBanner {}
            header { class: "sticky top-0 z-20 border-b border-base-300 bg-base-100/95 backdrop-blur",
                ShopNavbar {}
                // Small screens: the navbar has no room for search, so it gets its own row.
                div { class: "px-4 pb-2 sm:px-6 md:hidden", SearchBox { id: "search-mobile" } }
            }
            SystemNotices {}
            main { class: "mx-auto w-full max-w-[1200px] flex-1 px-4 py-6 sm:px-6 lg:py-8",
                Outlet::<Route> {}
            }
            footer { class: "border-t border-base-300 bg-base-200",
                div { class: "mx-auto flex max-w-[1200px] flex-col gap-2 px-4 py-6 text-sm text-base-content/60 sm:flex-row sm:justify-between sm:px-6",
                    span {
                        "TechDepó is a demo. Brands and products are fictional. "
                        Link { to: Route::DemoGuide {}, class: "link link-hover", "How this demo works" }
                    }
                    span { "Prices include VAT. Free shipping within the EU." }
                }
            }
        }
    }
}

#[component]
fn ShopNavbar() -> Element {
    let session = use_session();
    let cart = use_cart();
    let count = cart.count();
    let nav = navigator();

    rsx! {
        nav { class: "navbar mx-auto max-w-[1200px] gap-2 px-4 sm:px-6", aria_label: "Main",
            div { class: "navbar-start w-auto gap-1 lg:gap-4",
                // Mobile menu.
                div { class: "dropdown lg:hidden",
                    div { tabindex: "0", role: "button", class: "btn btn-ghost btn-square", aria_label: "Menu",
                        Icon { name: IconName::Menu, class: "size-5" }
                    }
                    ul { tabindex: "0", class: "dropdown-content menu z-30 mt-2 w-60 rounded-box border border-base-300 bg-base-100 p-2",
                        li { class: "menu-title", "Parts" }
                        for &category in Category::ALL {
                            li {
                                Link { to: Route::Category { category: category.as_str().to_string() },
                                    Icon { name: category_icon(category) }
                                    "{category_plural(category)}"
                                }
                            }
                        }
                        li { class: "menu-title", "Shop" }
                        li { Link { to: Route::Builder {}, Icon { name: IconName::Wrench }, "Build a PC" } }
                        li { Link { to: Route::support(), Icon { name: IconName::LifeBuoy }, "Support" } }
                    }
                }
                Logo {}
                div { class: "hidden items-center gap-1 lg:flex",
                    DropdownMenu {
                        DropdownMenuTrigger { class: "btn-ghost btn-sm gap-1",
                            "Parts"
                            Icon { name: IconName::ChevronDown, class: "size-3.5" }
                        }
                        DropdownMenuContent { class: "w-56",
                            for (index , &category) in Category::ALL.iter().enumerate() {
                                DropdownMenuItem::<Category> {
                                    value: category,
                                    index,
                                    on_select: move |c: Category| {
                                        nav.push(Route::Category { category: c.as_str().to_string() });
                                    },
                                    Icon { name: category_icon(category) }
                                    "{category_plural(category)}"
                                }
                            }
                        }
                    }
                    Link { to: Route::Builder {}, class: "btn btn-ghost btn-sm", "Build a PC" }
                    Link { to: Route::support(), class: "btn btn-ghost btn-sm", "Support" }
                }
            }
            div { class: "navbar-center hidden flex-1 justify-center px-2 md:flex",
                SearchBox { id: "search" }
            }
            div { class: "navbar-end ml-auto w-auto gap-1",
                ThemeToggle {}
                Link {
                    to: Route::Cart {},
                    class: "btn btn-ghost btn-square size-10",
                    aria_label: "Cart, {count} items",
                    div { class: "indicator",
                        Icon { name: IconName::Cart, class: "size-5" }
                        if count > 0 {
                            span { class: "badge badge-primary badge-xs indicator-item tabular-nums", "{count}" }
                        }
                    }
                }
                SignedIn {
                    if session.is_admin() {
                        Link { to: Route::AdminOverview {}, class: "btn btn-ghost btn-sm hidden sm:inline-flex", "Admin" }
                    }
                }
                UserControl {}
            }
        }
    }
}
