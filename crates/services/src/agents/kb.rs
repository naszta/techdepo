//! The knowledge base (`data/kb/*.md`), compiled into the binary like the catalog (support.md
//! §7). About fifteen short documents: buying guides (`guide-*`, for `ProductAdvisor`) and
//! policies (the rest, for `OrderAssistant`). Each starts with front matter (`slug`, `title`,
//! `summary`); `list_*` tools return those, `read_*` one document's text.

use std::sync::LazyLock;

/// Guides or policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// A buying guide (`guide-*`).
    Guide,
    /// A shop policy.
    Policy,
}

/// One document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// Slug (the file stem).
    pub slug: String,
    /// Title.
    pub title: String,
    /// One-sentence summary (what `list_*` shows the model).
    pub summary: String,
    /// The Markdown text after the front matter.
    pub body: &'static str,
    /// Guide or policy.
    pub kind: DocKind,
}

macro_rules! kb {
    ($($file:literal),+ $(,)?) => {
        /// `(file stem, contents)` of every document.
        const SOURCES: &[(&str, &str)] = &[
            $(($file, include_str!(concat!("../../../../data/kb/", $file, ".md")))),+
        ];
    };
}

kb![
    "cancellations",
    "contacting-support",
    "guide-bios-updates",
    "guide-cpu-coolers",
    "guide-ddr4-vs-ddr5",
    "guide-form-factors-and-clearance",
    "guide-gpu-power-connectors",
    "guide-m2-and-storage",
    "guide-psu-sizing",
    "invoices-and-vat",
    "payment",
    "refunds",
    "returns",
    "shipping",
    "warranty",
];

static DOCS: LazyLock<Vec<Doc>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .filter_map(|(stem, text)| parse(stem, text))
        .collect()
});

/// Splits front matter from the body. `None` if the front matter is missing.
fn parse(stem: &str, text: &'static str) -> Option<Doc> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let (front, body) = (&rest[..end], &rest[end + 5..]);
    let field = |name: &str| {
        front.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix(':')?.trim();
            Some(value.trim_matches('"').to_owned())
        })
    };
    let slug = field("slug").unwrap_or_else(|| stem.to_owned());
    Some(Doc {
        kind: if slug.starts_with("guide-") {
            DocKind::Guide
        } else {
            DocKind::Policy
        },
        title: field("title").unwrap_or_else(|| slug.clone()),
        summary: field("summary").unwrap_or_default(),
        body: body.trim_start(),
        slug,
    })
}

/// Every document of `kind`, by slug.
pub fn docs(kind: DocKind) -> impl Iterator<Item = &'static Doc> {
    DOCS.iter().filter(move |d| d.kind == kind)
}

/// The document `slug` of `kind`.
pub fn get(kind: DocKind, slug: &str) -> Option<&'static Doc> {
    let slug = slug.trim().trim_end_matches(".md");
    docs(kind).find(|d| d.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_compiled_in_and_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/kb");
        let mut stems: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| {
                let path = e.ok()?.path();
                (path.extension()? == "md")
                    .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
            })
            .collect();
        stems.sort();
        let listed: Vec<&str> = SOURCES.iter().map(|(s, _)| *s).collect();
        assert_eq!(stems, listed, "list every data/kb file in `kb!`");
        assert_eq!(DOCS.len(), SOURCES.len(), "every document has front matter");
        for doc in DOCS.iter() {
            assert!(SOURCES.iter().any(|(stem, _)| *stem == doc.slug));
            assert!(
                !doc.summary.is_empty() && doc.body.starts_with('#'),
                "{doc:?}"
            );
        }
        assert!(docs(DocKind::Guide).count() >= 5);
        let shipping = get(DocKind::Policy, "shipping").unwrap();
        assert_eq!(shipping.title, "Shipping and delivery");
        assert!(
            get(DocKind::Guide, "shipping").is_none(),
            "kinds stay apart"
        );
        assert_eq!(
            get(DocKind::Guide, "guide-ddr4-vs-ddr5").unwrap().title,
            "Buying guide: DDR4 vs DDR5 memory"
        );
    }
}
