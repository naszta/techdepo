//! The PC builder's current build: a local draft of product IDs in slot order, kept in
//! `localStorage` so it survives reloads and works signed out. "Save build" writes it to the
//! customer's saved builds (`POST`/`PUT /api/builds`), and the draft remembers which saved build
//! it belongs to, and whose: the draft outlives sign-out, so a saved build only counts for the
//! same org and user ([`BuildStore::saved_for`]); for anyone else the draft is a new build.
//!
//! Compatibility runs in the browser: the catalog is compiled in and `techdepo_compat` is pure,
//! so every change re-checks the build instantly with the same engine checkout uses.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::account::BuildDetail;
use techdepo_api_types::session::Me;
use techdepo_api_types::{BuildId, Category, Money, Product, ProductId};
use techdepo_catalog::Catalog;
use techdepo_compat::{Build, Issue, Report, Severity, Slot, check_build};

use crate::browser;

const DRAFT_KEY: &str = "techdepo.build";

/// The compiled-in catalog.
pub fn catalog() -> &'static Catalog {
    Catalog::global()
}

/// The saved build a draft belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRef {
    /// Build ID.
    pub id: BuildId,
    /// Its name.
    pub name: String,
    /// Whose it is ([`owner`]); `None` in drafts stored before this was recorded.
    #[serde(default)]
    pub owner: Option<String>,
}

/// Who a saved build belongs to: `<org>:<user>`.
pub fn owner(me: &Me) -> String {
    format!("{}:{}", me.org.id, me.user.id)
}

/// The current build as stored locally.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// Parts in slot order.
    pub parts: Vec<ProductId>,
    /// The saved build this draft was loaded from or last saved as.
    #[serde(default)]
    pub saved: Option<SavedRef>,
    /// Changed since it was last saved.
    #[serde(default)]
    pub dirty: bool,
    /// The idempotency key of creating this draft as a saved build: one per draft version, so a
    /// retried create (lost response) does not save it twice. Reset on every change.
    #[serde(default)]
    pub create_key: Option<String>,
}

/// The checked build, recomputed whenever the draft changes.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildView {
    /// Every part with its slot, in category order.
    pub slots: Vec<(Slot, &'static Product)>,
    /// The engine's report.
    pub report: Report,
    /// Required categories with no part yet.
    pub missing: Vec<Category>,
    /// Sum of the parts' prices.
    pub total: Money,
}

impl BuildView {
    fn of(parts: &[ProductId]) -> Self {
        let build = build_of(parts);
        Self {
            slots: build.parts(),
            report: check_build(&build),
            missing: build.missing_required(),
            total: build.total_price(),
        }
    }

    /// No parts yet.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// How a product would fit into the current build.
#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// The worst issue involving the product (`None`: compatible).
    pub worst: Option<Severity>,
    /// The issues involving the product, most severe first.
    pub issues: Vec<Issue>,
    /// The product is already part of the build.
    pub in_build: bool,
}

impl Fit {
    /// No errors (warnings and notes allowed).
    pub fn is_compatible(&self) -> bool {
        self.worst != Some(Severity::Error)
    }
}

/// A build from IDs, tolerant of unknown IDs (dropped) and repeated single slots (last wins).
pub fn build_of(parts: &[ProductId]) -> Build<'static> {
    let catalog = catalog();
    let mut build = Build::new();
    for id in parts {
        if let Some(product) = catalog.get(id) {
            build.add(product);
        }
    }
    build
}

/// How `product` fits into the build of `parts`: placed into its slot (replacing a single-slot
/// part, or added as another memory kit or drive), checked, and narrowed to the issues that
/// involve it. "Missing part" issues are left out: they are about the build, not the product.
///
/// `None` when the rest of the build is empty, so there is nothing to compare against.
pub fn fit(parts: &[ProductId], product: &'static Product) -> Option<Fit> {
    let mut build = build_of(parts);
    let in_build = build.slot_of(&product.id).is_some();
    if !in_build {
        build.add(product);
    }
    if build.len() < 2 {
        return None;
    }
    let report = check_build(&build);
    let issues: Vec<Issue> = report
        .issues
        .into_iter()
        .filter(|i| i.involves(&product.id) && !i.rule.is_absence())
        .collect();
    Some(Fit {
        worst: issues.iter().map(|i| i.severity).max(),
        issues,
        in_build,
    })
}

/// The current build, provided at the root.
#[derive(Clone, Copy)]
pub struct BuildStore {
    /// The draft (persisted on every change).
    pub draft: Signal<Draft>,
    /// The checked draft.
    pub view: Memo<BuildView>,
}

/// The build store from context.
pub fn use_build() -> BuildStore {
    use_context::<BuildStore>()
}

impl BuildStore {
    /// Restores the draft from `localStorage`. Call inside a component (creates a memo).
    pub fn load() -> Self {
        let draft = Signal::new(browser::load_json::<Draft>(DRAFT_KEY).unwrap_or_default());
        let view = Memo::new(move || BuildView::of(&draft.read().parts));
        Self { draft, view }
    }

    /// The part IDs in slot order.
    pub fn parts(&self) -> Vec<ProductId> {
        self.draft.read().parts.clone()
    }

    /// Whether `id` is in the build.
    pub fn contains(&self, id: &ProductId) -> bool {
        self.draft.read().parts.contains(id)
    }

    /// How `product` would fit (see [`fit`]).
    pub fn fit(&self, product: &'static Product) -> Option<Fit> {
        fit(&self.draft.read().parts, product)
    }

    fn edit(mut self, change: impl FnOnce(&mut Build<'static>)) {
        let mut draft = self.draft.write();
        let mut build = build_of(&draft.parts);
        change(&mut build);
        draft.parts = build.ids();
        draft.dirty = true;
        draft.create_key = None;
        browser::save_json(DRAFT_KEY, &*draft);
    }

    /// The saved build the draft belongs to, if it is `owner`'s (see [`owner`]).
    pub fn saved_for(&self, owner: Option<&str>) -> Option<SavedRef> {
        self.draft
            .read()
            .saved
            .clone()
            .filter(|s| owner.is_some() && s.owner.as_deref() == owner)
    }

    /// The key for creating the draft as a saved build (kept until the draft changes or is
    /// saved).
    pub fn create_key(mut self) -> String {
        let mut draft = self.draft.write();
        let key = draft.create_key.get_or_insert_with(browser::uuid).clone();
        browser::save_json(DRAFT_KEY, &*draft);
        key
    }

    /// The draft no longer belongs to a saved build (it was deleted, or is someone else's): the
    /// parts stay, the next save creates a new build.
    pub fn forget_saved(mut self) {
        let mut draft = self.draft.write();
        if draft.saved.take().is_some() {
            draft.dirty = true;
            draft.create_key = None;
            browser::save_json(DRAFT_KEY, &*draft);
        }
    }

    /// [`BuildStore::forget_saved`] if the draft belongs to the build `id`.
    pub fn forget_deleted(self, id: &BuildId) {
        let belongs = self
            .draft
            .peek()
            .saved
            .as_ref()
            .is_some_and(|s| &s.id == id);
        if belongs {
            self.forget_saved();
        }
    }

    /// Puts `product` into its slot: replaces a single-slot part, appends a memory kit or drive.
    pub fn place(self, product: &'static Product) {
        self.edit(|build| {
            build.add(product);
        });
    }

    /// Puts `product` into `slot` (for memory and storage, `index == len` appends).
    pub fn set(self, slot: Slot, product: &'static Product) {
        self.edit(|build| {
            let _ = build.set(slot, product);
        });
    }

    /// Empties `slot`.
    pub fn remove(self, slot: Slot) {
        self.edit(|build| {
            build.remove(slot);
        });
    }

    /// Starts a new, empty build.
    pub fn clear(mut self) {
        let draft = Draft::default();
        browser::save_json(DRAFT_KEY, &draft);
        self.draft.set(draft);
    }

    /// Replaces the draft with `owner`'s saved build.
    pub fn open(mut self, build: &BuildDetail, owner: Option<String>) {
        let draft = Draft {
            parts: build.product_ids.clone(),
            saved: Some(SavedRef {
                id: build.id.clone(),
                name: build.name.clone(),
                owner,
            }),
            dirty: false,
            create_key: None,
        };
        browser::save_json(DRAFT_KEY, &draft);
        self.draft.set(draft);
    }

    /// Records that the draft was saved as `owner`'s `build`.
    pub fn mark_saved(mut self, build: &BuildDetail, owner: Option<String>) {
        let mut draft = self.draft.write();
        draft.saved = Some(SavedRef {
            id: build.id.clone(),
            name: build.name.clone(),
            owner,
        });
        draft.dirty = false;
        draft.create_key = None;
        browser::save_json(DRAFT_KEY, &*draft);
    }
}
