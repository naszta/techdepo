//! Rich product content: one Markdown file per product with fixed `##` sections.

/// The `##` sections every content file has, in this order.
pub const REQUIRED_SECTIONS: [&str; 5] = ["Overview", "Who it's for", "Pros", "Cons", "FAQ"];

/// Long-form content for one product (`data/catalog/content/<product-id>.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductContent {
    markdown: String,
}

/// A `##` section: heading text and trimmed body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section<'a> {
    /// Heading text without the `## ` marker.
    pub heading: &'a str,
    /// Body, trimmed.
    pub body: &'a str,
}

/// One FAQ entry: a `### Question` heading and the text below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaqEntry<'a> {
    /// The question.
    pub question: &'a str,
    /// The answer, trimmed.
    pub answer: &'a str,
}

impl ProductContent {
    /// Wraps Markdown.
    pub fn new(markdown: impl Into<String>) -> Self {
        Self {
            markdown: markdown.into(),
        }
    }

    /// The raw Markdown.
    pub fn markdown(&self) -> &str {
        &self.markdown
    }

    /// Text before the first `##` heading, trimmed (should be empty).
    pub fn preamble(&self) -> &str {
        let end = heading_starts(&self.markdown, "## ")
            .next()
            .unwrap_or(self.markdown.len());
        self.markdown[..end].trim()
    }

    /// All `##` sections in document order.
    pub fn sections(&self) -> Vec<Section<'_>> {
        split_headings(&self.markdown, "## ")
            .into_iter()
            .map(|(heading, body)| Section { heading, body })
            .collect()
    }

    /// The body of the section titled `heading`.
    pub fn section(&self, heading: &str) -> Option<&str> {
        self.sections()
            .into_iter()
            .find(|s| s.heading == heading)
            .map(|s| s.body)
    }

    /// `## Overview`.
    pub fn overview(&self) -> Option<&str> {
        self.section("Overview")
    }

    /// `## Who it's for`.
    pub fn who_its_for(&self) -> Option<&str> {
        self.section("Who it's for")
    }

    /// Bullet items of `## Pros`.
    pub fn pros(&self) -> Vec<&str> {
        self.section("Pros").map(bullets).unwrap_or_default()
    }

    /// Bullet items of `## Cons`.
    pub fn cons(&self) -> Vec<&str> {
        self.section("Cons").map(bullets).unwrap_or_default()
    }

    /// `### Question` entries of `## FAQ`.
    pub fn faq(&self) -> Vec<FaqEntry<'_>> {
        self.section("FAQ")
            .map(|body| {
                split_headings(body, "### ")
                    .into_iter()
                    .map(|(question, answer)| FaqEntry { question, answer })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Byte offsets of lines starting with `marker`.
fn heading_starts<'a>(text: &'a str, marker: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut offset = 0;
    text.split_inclusive('\n').filter_map(move |line| {
        let start = offset;
        offset += line.len();
        line.starts_with(marker).then_some(start)
    })
}

/// Splits `text` at lines starting with `marker` into (heading, trimmed body) pairs.
fn split_headings<'a>(text: &'a str, marker: &str) -> Vec<(&'a str, &'a str)> {
    let starts: Vec<usize> = heading_starts(text, marker).collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            let block = &text[start + marker.len()..end];
            let (heading, body) = block.split_once('\n').unwrap_or((block, ""));
            (heading.trim(), body.trim())
        })
        .collect()
}

/// Items of a `- ` bullet list (continuation lines are not supported).
fn bullets(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "## Overview\nA fast chip.\n\n## Who it's for\nGamers.\n\n## Pros\n- Fast\n- Cheap\n\n## Cons\n- Hot\n\n## FAQ\n### Does it fit AM5?\nYes.\n\n### Cooler included?\nNo, buy one.\n";

    #[test]
    fn parses_sections() {
        let c = ProductContent::new(SAMPLE);
        assert_eq!(c.preamble(), "");
        let headings: Vec<_> = c.sections().iter().map(|s| s.heading).collect();
        assert_eq!(headings, REQUIRED_SECTIONS);
        assert_eq!(c.overview(), Some("A fast chip."));
        assert_eq!(c.pros(), vec!["Fast", "Cheap"]);
        assert_eq!(c.cons(), vec!["Hot"]);
        let faq = c.faq();
        assert_eq!(faq.len(), 2);
        assert_eq!(faq[1].question, "Cooler included?");
        assert_eq!(faq[1].answer, "No, buy one.");
    }

    #[test]
    fn preamble_is_reported() {
        let c = ProductContent::new("# Title\n\n## Overview\nText\n");
        assert_eq!(c.preamble(), "# Title");
        assert_eq!(c.overview(), Some("Text"));
    }
}
