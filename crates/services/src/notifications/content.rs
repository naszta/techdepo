//! The content model of an email and the shared layout that renders it.
//!
//! Every template ([`super::templates`]) is a pure function that returns an [`EmailContent`]:
//! subject, heading and a list of [`Block`]s. One askama layout renders it twice, as HTML
//! (`templates/email/layout.html`: neutral greys, one indigo accent, the demo banner) and as the
//! plain-text alternative (`templates/email/layout.txt`), so every email looks the same and the
//! copy lives in one place per template.

use askama::Template;
use serde::{Deserialize, Serialize};

/// The thin banner on top of every email (design.md: neutral, not alarming).
pub const DEMO_BANNER: &str = "Demo shop – not a real store";

/// One email before the layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailContent {
    /// Template name (`email_log.template`), e.g. `order_confirmation`.
    pub template: String,
    /// Subject line.
    pub subject: String,
    /// The inbox preview text.
    pub preheader: String,
    /// The headline inside the email.
    pub heading: String,
    /// Body blocks, in order.
    pub blocks: Vec<Block>,
    /// The one button, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cta: Option<Cta>,
}

impl EmailContent {
    /// Content with a template name, subject and heading; the preheader defaults to the
    /// heading.
    pub fn new(
        template: impl Into<String>,
        subject: impl Into<String>,
        heading: impl Into<String>,
    ) -> Self {
        let heading = heading.into();
        Self {
            template: template.into(),
            subject: subject.into(),
            preheader: heading.clone(),
            heading,
            blocks: Vec::new(),
            cta: None,
        }
    }

    /// Sets the preview text.
    #[must_use]
    pub fn preheader(mut self, text: impl Into<String>) -> Self {
        self.preheader = text.into();
        self
    }

    /// Appends a paragraph.
    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Text(text.into()));
        self
    }

    /// Appends free text (an agent's or a person's reply): one paragraph per blank-line
    /// separated chunk.
    #[must_use]
    pub fn body(mut self, body: &str) -> Self {
        for para in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            self.blocks.push(Block::Text(para.to_owned()));
        }
        self
    }

    /// Appends a highlighted note.
    #[must_use]
    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Note(text.into()));
        self
    }

    /// Appends a quotation (the customer's words).
    #[must_use]
    pub fn quote(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Quote(text.into()));
        self
    }

    /// Appends a label/value list.
    #[must_use]
    pub fn facts(mut self, facts: Vec<Fact>) -> Self {
        if !facts.is_empty() {
            self.blocks.push(Block::Facts(facts));
        }
        self
    }

    /// Appends an item table with its total rows (the last one is printed bold).
    #[must_use]
    pub fn items(mut self, items: Vec<ItemLine>, totals: Vec<Fact>) -> Self {
        self.blocks.push(Block::Items { items, totals });
        self
    }

    /// Sets the button.
    #[must_use]
    pub fn cta(mut self, label: impl Into<String>, url: impl Into<String>) -> Self {
        self.cta = Some(Cta {
            label: label.into(),
            url: url.into(),
        });
        self
    }
}

/// A block of the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Block {
    /// A paragraph (single line breaks are kept).
    Text(String),
    /// A highlighted note.
    Note(String),
    /// Quoted text.
    Quote(String),
    /// Label/value rows.
    Facts(Vec<Fact>),
    /// Order lines and totals.
    Items {
        /// Lines.
        items: Vec<ItemLine>,
        /// Total rows.
        totals: Vec<Fact>,
    },
}

/// A label and its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// Label.
    pub label: String,
    /// Value.
    pub value: String,
}

impl Fact {
    /// A fact.
    pub fn new(label: impl Into<String>, value: impl ToString) -> Self {
        Self {
            label: label.into(),
            value: value.to_string(),
        }
    }
}

/// One order line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemLine {
    /// Product name.
    pub name: String,
    /// SKU (empty: not printed).
    pub sku: String,
    /// Quantity.
    pub quantity: u32,
    /// Line total, formatted.
    pub amount: String,
}

/// A button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cta {
    /// Label.
    pub label: String,
    /// Absolute URL.
    pub url: String,
}

/// What the layout prints around the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The org's support address (`support+<public_id>@<domain>`).
    pub support_address: String,
    /// Why the reader gets this email.
    pub footer: String,
}

/// A rendered email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    /// Template name.
    pub template: String,
    /// Subject.
    pub subject: String,
    /// Plain-text body.
    pub text: String,
    /// HTML body.
    pub html: String,
}

/// A block flattened for the templates (askama matches on plain fields more simply than on
/// enum variants).
struct BlockView {
    kind: &'static str,
    text: String,
    rows: Vec<Fact>,
    items: Vec<ItemLine>,
}

impl From<&Block> for BlockView {
    fn from(block: &Block) -> Self {
        let empty = |kind| Self {
            kind,
            text: String::new(),
            rows: Vec::new(),
            items: Vec::new(),
        };
        match block {
            Block::Text(t) => Self {
                text: t.clone(),
                ..empty("text")
            },
            Block::Note(t) => Self {
                text: t.clone(),
                ..empty("note")
            },
            Block::Quote(t) => Self {
                text: t.clone(),
                ..empty("quote")
            },
            Block::Facts(rows) => Self {
                rows: rows.clone(),
                ..empty("facts")
            },
            Block::Items { items, totals } => Self {
                rows: totals.clone(),
                items: items.clone(),
                ..empty("items")
            },
        }
    }
}

#[derive(Template)]
#[template(path = "email/layout.html")]
struct HtmlLayout<'a> {
    subject: &'a str,
    preheader: &'a str,
    banner: &'a str,
    heading: &'a str,
    blocks: &'a [BlockView],
    cta: &'a Option<Cta>,
    sign_off: &'a str,
    footer: &'a str,
    support_address: &'a str,
}

#[derive(Template)]
#[template(path = "email/layout.txt")]
struct TextLayout<'a> {
    banner: &'a str,
    heading: &'a str,
    blocks: &'a [BlockView],
    cta: &'a Option<Cta>,
    sign_off: &'a str,
    footer: &'a str,
    support_address: &'a str,
}

/// The sign-off under every email.
pub const SIGN_OFF: &str = "The TechDepó team";

/// Renders `content` in the layout, as HTML and as text.
pub fn render(content: &EmailContent, frame: &Frame) -> Result<Rendered, askama::Error> {
    let blocks: Vec<BlockView> = content.blocks.iter().map(BlockView::from).collect();
    let html = HtmlLayout {
        subject: &content.subject,
        preheader: &content.preheader,
        banner: DEMO_BANNER,
        heading: &content.heading,
        blocks: &blocks,
        cta: &content.cta,
        sign_off: SIGN_OFF,
        footer: &frame.footer,
        support_address: &frame.support_address,
    }
    .render()?;
    let text = TextLayout {
        banner: DEMO_BANNER,
        heading: &content.heading,
        blocks: &blocks,
        cta: &content.cta,
        sign_off: SIGN_OFF,
        footer: &frame.footer,
        support_address: &frame.support_address,
    }
    .render()?;
    Ok(Rendered {
        template: content.template.clone(),
        subject: content.subject.clone(),
        text: tidy_text(&text),
        html,
    })
}

/// Collapses runs of blank lines left by the template's control flow.
fn tidy_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            support_address: "support+k3x9q2ab@techdepo.dev".into(),
            footer: "You get this email because you ordered from the TechDepó demo.".into(),
        }
    }

    #[test]
    fn renders_html_and_text() {
        let content = EmailContent::new("test", "Order TD-1042 confirmed", "Thanks <Ada>")
            .text("Line one\nline two")
            .facts(vec![Fact::new("Order", "TD-1042")])
            .items(
                vec![ItemLine {
                    name: "Voltaris V5 8600".into(),
                    sku: "CPU-VOL-8600".into(),
                    quantity: 2,
                    amount: "€254.00".into(),
                }],
                vec![Fact::new("Total", "€254.00")],
            )
            .note("Demo note")
            .cta("View order", "http://localhost:8787/account/orders/TD-1042");
        let out = render(&content, &frame()).unwrap();
        assert!(out.html.contains("Thanks &#60;Ada&#62;"), "escaped heading");
        assert!(out.html.contains(DEMO_BANNER));
        assert!(out.html.contains("#4f46e5"), "the one accent");
        assert!(out.html.contains("CPU-VOL-8600"));
        assert!(out.text.starts_with(DEMO_BANNER));
        assert!(out.text.contains("Thanks <Ada>"), "text is not escaped");
        assert!(out.text.contains("Order: TD-1042"));
        assert!(
            out.text
                .contains("2 x Voltaris V5 8600 (CPU-VOL-8600)  €254.00")
        );
        assert!(
            out.text
                .contains("View order: http://localhost:8787/account/orders/TD-1042")
        );
        assert!(!out.text.contains("\n\n\n"));
    }

    #[test]
    fn free_text_bodies_become_paragraphs() {
        let content =
            EmailContent::new("reply", "Re: x", "Hello").body("First.\n\n\nSecond\nline.");
        assert_eq!(
            content.blocks,
            vec![
                Block::Text("First.".into()),
                Block::Text("Second\nline.".into())
            ]
        );
    }
}
