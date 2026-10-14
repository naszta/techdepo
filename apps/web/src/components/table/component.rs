use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's size axis for a table.
///
/// [`TableSize::Default`] emits no modifier and renders at the same medium
/// size as daisyUI's explicit `table-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TableSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl TableSize {
    /// Every value of this Axis, from smallest to largest.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "table-xs",
            Self::Sm => "table-sm",
            Self::Default => "",
            Self::Lg => "table-lg",
            Self::Xl => "table-xl",
        }
    }
}

/// Whether a table uses daisyUI's zebra-striped body rows.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TableZebra {
    #[default]
    Default,
    Zebra,
}

impl TableZebra {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Zebra];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Zebra => "table-zebra",
        }
    }
}

/// Whether a table pins its header and footer rows inside a scrolling ancestor.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TableRowPinning {
    #[default]
    Default,
    Pinned,
}

impl TableRowPinning {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Pinned];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Pinned => "table-pin-rows",
        }
    }
}

/// Whether a table pins its header cells inside a horizontally scrolling ancestor.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TableColumnPinning {
    #[default]
    Default,
    Pinned,
}

impl TableColumnPinning {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Pinned];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Pinned => "table-pin-cols",
        }
    }
}

/// A native table carrying daisyUI's `table` classes.
///
/// Size, zebra striping, row pinning and column pinning are independent Axes.
/// Classes passed by the caller concatenate with the table's own; every other
/// native table attribute the caller passes overrides the table's.
#[component]
pub fn Table(
    /// daisyUI's size axis.
    #[props(default)]
    size: TableSize,
    /// Whether body rows are zebra-striped.
    #[props(default)]
    zebra: TableZebra,
    /// Whether header and footer rows stick in place inside a scrolling ancestor
    /// the caller provides.
    #[props(default)]
    row_pinning: TableRowPinning,
    /// Whether header cells stick in place inside a horizontally scrolling
    /// ancestor the caller provides.
    #[props(default)]
    column_pinning: TableColumnPinning,
    #[props(extends = GlobalAttributes)]
    #[props(extends = table)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let size = size.class();
    let zebra = zebra.class();
    let row_pinning = row_pinning.class();
    let column_pinning = column_pinning.class();

    let base = attributes!(table {
        class: "table {size} {zebra} {row_pinning} {column_pinning}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        table { ..merged, {children} }
    }
}

/// A native `caption` that names and describes a [`Table`].
#[component]
pub fn TableCaption(
    #[props(extends = GlobalAttributes)]
    #[props(extends = caption)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        caption { ..attributes, {children} }
    }
}

/// A native `thead` containing a [`Table`]'s column headings.
#[component]
pub fn TableHeader(
    #[props(extends = GlobalAttributes)]
    #[props(extends = thead)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        thead { ..attributes, {children} }
    }
}

/// A native `tbody` containing a [`Table`]'s data rows.
#[component]
pub fn TableBody(
    #[props(extends = GlobalAttributes)]
    #[props(extends = tbody)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        tbody { ..attributes, {children} }
    }
}

/// A native `tfoot` containing a [`Table`]'s footer rows.
#[component]
pub fn TableFooter(
    #[props(extends = GlobalAttributes)]
    #[props(extends = tfoot)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        tfoot { ..attributes, {children} }
    }
}

/// A native `tr` inside a table section.
#[component]
pub fn TableRow(
    #[props(extends = GlobalAttributes)]
    #[props(extends = tr)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        tr { ..attributes, {children} }
    }
}

/// A native `th` exposing header-cell attributes such as `scope`, `colspan`
/// and `rowspan`.
#[component]
pub fn TableHeaderCell(
    #[props(extends = GlobalAttributes)]
    #[props(extends = th)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        th { ..attributes, {children} }
    }
}

/// A native `td` exposing data-cell attributes such as `colspan` and
/// `rowspan`.
#[component]
pub fn TableCell(
    #[props(extends = GlobalAttributes)]
    #[props(extends = td)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        td { ..attributes, {children} }
    }
}
