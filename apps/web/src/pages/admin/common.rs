//! Building blocks shared by the admin pages: paged lists with "Load more", compact filters,
//! links to admin records, definition lists and the evidence list.

use std::future::Future;
use std::str::FromStr;

use dioxus::prelude::*;
use techdepo_api_types::tasks::hand_back_intent;
use techdepo_api_types::vocab::RefKind;
use techdepo_api_types::{
    ApiError, CustomerRef, EntityRef, Evidence, Intent, OrderNumber, OrderRef, Page, TicketRef,
};

use crate::routes::Route;
use crate::ui::format::{datetime, duration};
use crate::ui::{
    ErrorAlert, Icon, IconName, MoneyText, Mono, OrderStatusBadge, PageSkeleton, ShortId,
    TicketStatusBadge,
};

// ---------------------------------------------------------------------------------------------
// Paged lists
// ---------------------------------------------------------------------------------------------

/// A cursor-paged list: the first page is a resource (it re-runs when the signals the fetch
/// reads change, i.e. the filters), later pages are appended by [`Paged::load_more`].
pub struct Paged<T: 'static> {
    first: Resource<Result<Page<T>, ApiError>>,
    more: Signal<Vec<T>>,
    cursor: Signal<Option<String>>,
    loading: Signal<bool>,
    error: Signal<Option<ApiError>>,
    fetch: Callback<Option<String>, ()>,
}

impl<T> Clone for Paged<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Paged<T> {}

/// A paged list read with `fetch(cursor)`. Signals read inside `fetch` are its filters.
pub fn use_paged<T, F, Fut>(fetch: F) -> Paged<T>
where
    T: Clone + 'static,
    F: Fn(Option<String>) -> Fut + Copy + 'static,
    Fut: Future<Output = Result<Page<T>, ApiError>> + 'static,
{
    let mut more = use_signal(Vec::<T>::new);
    let mut cursor = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<ApiError>);
    let first = use_resource(move || async move {
        let page = fetch(None).await;
        more.set(Vec::new());
        error.set(None);
        cursor.set(page.as_ref().ok().and_then(|p| p.next_cursor.clone()));
        page
    });
    let next = use_callback(move |after: Option<String>| {
        spawn(async move {
            loading.set(true);
            match fetch(after).await {
                Ok(page) => {
                    more.write().extend(page.items);
                    cursor.set(page.next_cursor);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });
    Paged {
        first,
        more,
        cursor,
        loading,
        error,
        fetch: next,
    }
}

impl<T: Clone + 'static> Paged<T> {
    /// `None` while the first page loads; then every loaded item, or the first page's error.
    pub fn items(&self) -> Option<Result<Vec<T>, ApiError>> {
        match &*self.first.read() {
            None => None,
            Some(Err(e)) => Some(Err(e.clone())),
            Some(Ok(page)) => {
                let mut items = page.items.clone();
                items.extend(self.more.read().iter().cloned());
                Some(Ok(items))
            }
        }
    }

    /// Reload from the first page.
    pub fn restart(&mut self) {
        self.first.restart();
    }

    /// Skeleton, error or `body(items)`, followed by the "Load more" footer.
    pub fn render(&self, body: impl FnOnce(Vec<T>) -> Element) -> Element {
        let mut this = *self;
        match self.items() {
            None => rsx! { PageSkeleton { rows: 2 } },
            Some(Err(error)) => rsx! { ErrorAlert { error, on_retry: move |_| this.restart() } },
            Some(Ok(items)) => {
                let (cursor, loading, error, fetch) =
                    (self.cursor, self.loading, self.error, self.fetch);
                rsx! {
                    {body(items)}
                    if let Some(error) = error() {
                        div { class: "mt-3", ErrorAlert { error } }
                    }
                    if cursor().is_some() {
                        div { class: "mt-3 flex justify-center",
                            button {
                                class: "btn btn-sm",
                                disabled: loading(),
                                onclick: move |_| fetch.call(cursor()),
                                if loading() { span { class: "loading loading-spinner loading-xs" } }
                                "Load more"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------------------------

/// `(wire name, label)` pairs of a labeled enum, for a [`SelectFilter`].
pub fn options<E: Copy>(
    all: &[E],
    wire: impl Fn(E) -> &'static str,
    label: impl Fn(E) -> &'static str,
) -> Vec<(String, String)> {
    all.iter()
        .map(|e| (wire(*e).to_owned(), label(*e).to_owned()))
        .collect()
}

/// `Some(value)` for a selected filter, `None` for "all" (the empty string).
pub fn parsed<T: FromStr>(value: &str) -> Option<T> {
    if value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

/// `Some(trimmed)` unless blank.
pub fn non_blank(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// A row of filters above a list.
#[component]
pub fn FilterBar(children: Element) -> Element {
    rsx! {
        div { class: "mb-4 flex flex-wrap items-end gap-3", {children} }
    }
}

/// A labelled `select` whose empty option means "all".
#[component]
pub fn SelectFilter(
    label: String,
    value: Signal<String>,
    options: Vec<(String, String)>,
    #[props(default = "All".to_string())] all: String,
) -> Element {
    let mut value = value;
    rsx! {
        label { class: "flex flex-col gap-1",
            span { class: "text-xs text-base-content/60", "{label}" }
            select {
                class: "select select-sm w-44",
                value: "{value}",
                onchange: move |e| value.set(e.value()),
                option { value: "", selected: value.read().is_empty(), "{all}" }
                for (wire , text) in options {
                    option { key: "{wire}", value: "{wire}", selected: *value.read() == wire, "{text}" }
                }
            }
        }
    }
}

/// What the customer wants, for a hand-back to the AI: empty lets the AI classify again.
#[component]
pub fn HandBackIntent(value: Signal<String>, #[props(default)] class: String) -> Element {
    let mut value = value;
    rsx! {
        select {
            class: "select select-sm {class}",
            aria_label: "What the customer wants",
            onchange: move |e| value.set(e.value()),
            option { value: "", selected: value.read().is_empty(), "Let the AI classify it again" }
            for intent in Intent::ALL.iter().copied().filter(|i| hand_back_intent(*i)) {
                option { key: "{intent.as_str()}", value: "{intent.as_str()}", selected: *value.read() == intent.as_str(), "{intent.label()}" }
            }
        }
    }
}

/// The intent a [`HandBackIntent`] holds.
pub fn hand_back_choice(value: &str) -> Option<Intent> {
    value.parse().ok()
}

/// A labelled toggle.
#[component]
pub fn ToggleFilter(label: String, value: Signal<bool>) -> Element {
    let mut value = value;
    rsx! {
        label { class: "flex h-8 cursor-pointer items-center gap-2 self-end",
            input {
                r#type: "checkbox",
                class: "toggle toggle-sm",
                checked: value(),
                onchange: move |e| value.set(e.checked()),
            }
            span { "{label}" }
        }
    }
}

/// A search box that applies on Enter (and on blur), not on every keystroke.
#[component]
pub fn SearchFilter(label: String, placeholder: String, value: Signal<String>) -> Element {
    let mut value = value;
    let mut draft = use_signal(|| value.peek().clone());
    rsx! {
        form {
            class: "flex flex-col gap-1",
            onsubmit: move |e| {
                e.prevent_default();
                value.set(draft());
            },
            span { class: "text-xs text-base-content/60", "{label}" }
            label { class: "input input-sm w-64",
                Icon { name: IconName::Search, class: "size-3.5 opacity-60" }
                input {
                    r#type: "search",
                    class: "grow",
                    placeholder: "{placeholder}",
                    value: "{draft}",
                    oninput: move |e| draft.set(e.value()),
                    onchange: move |e| value.set(e.value()),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Links and references
// ---------------------------------------------------------------------------------------------

/// "← label" back to a list.
#[component]
pub fn BackLink(to: Route, label: String) -> Element {
    rsx! {
        Link { to, class: "mb-3 inline-flex items-center gap-1 text-sm text-base-content/70 hover:text-base-content",
            Icon { name: IconName::ArrowLeft, class: "size-3.5" }
            "{label}"
        }
    }
}

/// An order number linking to its admin page.
#[component]
pub fn OrderLink(number: OrderNumber) -> Element {
    rsx! {
        Link { to: Route::AdminOrder { number: number.to_string() }, class: "link link-hover whitespace-nowrap",
            Mono { "{number}" }
        }
    }
}

/// An order reference: number and status.
#[component]
pub fn OrderRefView(order: OrderRef) -> Element {
    rsx! {
        span { class: "inline-flex flex-wrap items-center gap-2",
            OrderLink { number: order.number }
            OrderStatusBadge { status: order.status }
        }
    }
}

/// A ticket reference: ID, subject and status.
#[component]
pub fn TicketRefView(ticket: TicketRef) -> Element {
    rsx! {
        Link {
            to: Route::admin_ticket(&ticket.id),
            class: "flex items-center gap-2 rounded-field border border-base-300 p-2 hover:bg-base-200",
            div { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate font-medium", "{ticket.subject}" }
                Mono { class: "text-xs text-base-content/60", "{ticket.id}" }
            }
            TicketStatusBadge { status: ticket.status }
        }
    }
}

/// A customer: name (or email) linking to their admin page, email underneath.
#[component]
pub fn CustomerCell(customer: CustomerRef) -> Element {
    rsx! {
        div { class: "flex min-w-0 flex-col",
            Link {
                to: Route::AdminCustomer { user_id: customer.user_id.to_string() },
                class: "link link-hover truncate",
                {customer.name.clone().unwrap_or_else(|| customer.email.clone())}
            }
            if customer.name.is_some() {
                span { class: "truncate text-xs text-base-content/60", "{customer.email}" }
            }
        }
    }
}

/// The admin page of a referenced record, where there is one.
pub fn entity_route(entity: &EntityRef) -> Option<Route> {
    Some(match entity.kind {
        RefKind::Order => Route::AdminOrder {
            number: entity.label.clone(),
        },
        RefKind::Ticket => Route::admin_ticket(&entity.id),
        RefKind::Refund => Route::AdminRefund {
            id: entity.id.clone(),
        },
        RefKind::Investigation => Route::AdminInvestigation {
            id: entity.id.clone(),
        },
        RefKind::HumanTask => Route::AdminTask {
            id: entity.id.clone(),
        },
        RefKind::Email => Route::AdminEmail {
            id: entity.id.clone(),
        },
        RefKind::Invoice => Route::AdminInvoices {},
        RefKind::Payment => Route::AdminPayments {},
        RefKind::Shipment | RefKind::AgentRun => return None,
    })
}

/// A referenced record's label, linked when it has an admin page.
#[component]
pub fn EntityLink(entity: EntityRef) -> Element {
    match entity_route(&entity) {
        Some(to) => rsx! {
            Link { to, class: "link link-hover", Mono { "{entity.label}" } }
        },
        None => rsx! { Mono { "{entity.label}" } },
    }
}

// ---------------------------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------------------------

/// A bordered, horizontally scrolling container for a compact table.
#[component]
pub fn TableWrap(children: Element) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-box border border-base-300 bg-base-100",
            {children}
        }
    }
}

/// A definition list of label/value rows.
#[component]
pub fn Facts(children: Element) -> Element {
    rsx! {
        dl { class: "grid grid-cols-[minmax(0,9rem)_1fr] gap-x-3 gap-y-1.5", {children} }
    }
}

/// One row of [`Facts`].
#[component]
pub fn Fact(label: String, children: Element) -> Element {
    rsx! {
        dt { class: "text-base-content/60", "{label}" }
        dd { class: "min-w-0 break-words", {children} }
    }
}

/// A one-line note inside a card ("Nothing here yet").
#[component]
pub fn Muted(children: Element) -> Element {
    rsx! {
        p { class: "text-base-content/60", {children} }
    }
}

// ---------------------------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------------------------

/// Evidence gathered by code (support.md §8): what a person decides on.
#[component]
pub fn EvidenceList(evidence: Vec<Evidence>) -> Element {
    if evidence.is_empty() {
        return rsx! { Muted { "No evidence attached." } };
    }
    rsx! {
        ul { class: "flex flex-col gap-2",
            for (i , item) in evidence.into_iter().enumerate() {
                li { key: "{i}", class: "flex gap-2.5 rounded-field border border-base-300 p-2.5",
                    Icon { name: evidence_icon(&item), class: "mt-0.5 size-4 text-base-content/60" }
                    div { class: "flex min-w-0 flex-1 flex-col gap-0.5", {evidence_body(item)} }
                }
            }
        }
    }
}

fn evidence_icon(evidence: &Evidence) -> IconName {
    match evidence {
        Evidence::DuplicateCharge { .. } => IconName::CreditCard,
        Evidence::InvoiceMissingAddress { .. } => IconName::Receipt,
        Evidence::OrderState { .. } => IconName::Package,
        Evidence::CarrierTrace { .. } => IconName::Truck,
        Evidence::CustomerMessage { .. } => IconName::Mail,
        Evidence::OrderEvent { .. } => IconName::Clock,
        Evidence::Note { .. } => IconName::Info,
    }
}

fn evidence_body(evidence: Evidence) -> Element {
    match evidence {
        Evidence::DuplicateCharge {
            order_number,
            first,
            duplicate,
            amount,
            seconds_apart,
        } => rsx! {
            span { class: "font-medium",
                "Two charges of " MoneyText { amount } " on " OrderLink { number: order_number }
            }
            span { class: "text-xs text-base-content/60",
                ShortId { id: first.to_string() } " and " ShortId { id: duplicate.to_string() } ", {apart(seconds_apart)} apart"
            }
        },
        Evidence::InvoiceMissingAddress { number, .. } => rsx! {
            span { class: "font-medium", "Invoice " Mono { "{number}" } " has no billing address" }
        },
        Evidence::OrderState {
            order_number,
            status,
            workdays_since_ship,
        } => rsx! {
            span { class: "inline-flex flex-wrap items-center gap-2 font-medium",
                OrderLink { number: order_number }
                OrderStatusBadge { status }
            }
            if let Some(n) = workdays_since_ship {
                span { class: "text-xs text-base-content/60", "{n} workday{plural(n)} since shipment" }
            }
        },
        Evidence::CarrierTrace {
            tracking_number,
            status,
            outcome,
            ..
        } => rsx! {
            span { class: "font-medium", "Carrier trace on " Mono { "{tracking_number}" } ": {status.label()}" }
            if let Some(outcome) = outcome {
                span { class: "text-xs text-base-content/60",
                    match outcome {
                        techdepo_api_types::LostOutcome::Resurfaces => "The parcel was found.",
                        techdepo_api_types::LostOutcome::Gone => "No information: the parcel is gone.",
                    }
                }
            }
        },
        Evidence::CustomerMessage {
            ticket_id,
            seq,
            excerpt,
        } => rsx! {
            span { class: "text-xs text-base-content/60",
                "Customer message #{seq} on "
                Link { to: Route::admin_ticket(&ticket_id), class: "link link-hover", Mono { "{ticket_id}" } }
            }
            blockquote { class: "whitespace-pre-wrap border-l-2 border-base-300 pl-2 italic", "{excerpt}" }
        },
        Evidence::OrderEvent {
            order_number,
            event,
            at,
            ..
        } => rsx! {
            span { class: "font-medium", "{event.label()}" }
            span { class: "text-xs text-base-content/60",
                OrderLink { number: order_number } " · {datetime(at)}"
            }
        },
        Evidence::Note { text } => rsx! { span { "{text}" } },
    }
}

/// `4 s` under a minute, otherwise `2 min`, `1 h 5 min`.
pub fn apart(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else {
        duration(secs)
    }
}

/// `""` or `"s"`.
pub const fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ---------------------------------------------------------------------------------------------
// Small forms (dialogs and inline edits)
// ---------------------------------------------------------------------------------------------

/// The message for `field` among validation errors.
pub fn error_for(errors: &[techdepo_api_types::FieldError], field: &str) -> Option<String> {
    errors
        .iter()
        .find(|e| e.field == field)
        .map(|e| e.message.clone())
}

/// `229.90`, `229,90` or `229` euros as money; `None` when it is not an amount.
pub fn parse_euros(text: &str) -> Option<techdepo_api_types::Money> {
    let text = text.trim().replace(['€', ' '], "").replace(',', ".");
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, f),
        None => (text.as_str(), ""),
    };
    if whole.is_empty() || frac.len() > 2 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let frac: i64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<2}").parse().ok()?
    };
    Some(techdepo_api_types::Money::from_cents(
        whole.checked_mul(100)? + frac,
    ))
}

/// Cents as a plain amount for an input (`229.90`).
pub fn euros_input(amount: techdepo_api_types::Money) -> String {
    let cents = amount.cents();
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}

/// A labelled text input or textarea bound to a signal, with its validation message.
#[component]
pub fn TextField(
    label: String,
    value: Signal<String>,
    #[props(default)] error: Option<String>,
    #[props(default)] multiline: bool,
    #[props(default = 3)] rows: u32,
    #[props(default)] placeholder: String,
    #[props(default)] hint: Option<String>,
    #[props(default = "w-full".to_string())] class: String,
) -> Element {
    let mut value = value;
    rsx! {
        label { class: "flex flex-col gap-1 {class}",
            span { class: "font-medium", "{label}" }
            if multiline {
                textarea {
                    class: "textarea w-full",
                    class: if error.is_some() { "textarea-error" },
                    rows: "{rows}",
                    placeholder: "{placeholder}",
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                }
            } else {
                input {
                    class: "input w-full",
                    class: if error.is_some() { "input-error" },
                    placeholder: "{placeholder}",
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                }
            }
            if let Some(hint) = hint {
                span { class: "text-xs text-base-content/60", "{hint}" }
            }
            if let Some(error) = error {
                span { class: "text-xs text-error", "{error}" }
            }
        }
    }
}
