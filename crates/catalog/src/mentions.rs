//! Product names in free text: content files, knowledge-base articles and agent replies.
//!
//! A mention is a brand followed by capitalised words or numbers ("Nimbus Stratus DDR5-6000
//! 32 GB"). It names a product when it is the product's full name or a word-boundary prefix of
//! exactly one name. A prefix of several names ("Halcyon HX 7070" for the Twin and the SFF) is
//! ambiguous: text that customers or agents read must not use one.

use crate::catalog::Catalog;
use crate::product::Product;

/// What a mention refers to.
#[derive(Debug, Clone, PartialEq)]
pub enum MentionTarget<'c> {
    /// Exactly one product: the full name, or a prefix only it has.
    Product(&'c Product),
    /// A prefix of several product names.
    Ambiguous(Vec<&'c Product>),
}

/// A product mention found in a text.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductMention<'c> {
    /// The mention, with whitespace (line breaks included) collapsed to single spaces.
    pub text: String,
    /// Byte offset of the mention in the scanned text.
    pub offset: usize,
    /// What it refers to.
    pub target: MentionTarget<'c>,
}

impl Catalog {
    /// Every product mention in `text`, in order.
    ///
    /// A brand followed only by words that start no product name ("Helion CPUs", "Nimbus DDR5
    /// kits") is a brand reference, not a product mention, and is not returned.
    pub fn product_mentions(&self, text: &str) -> Vec<ProductMention<'_>> {
        let words = words(text);
        let mut mentions = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let found = self
                .is_brand(words[i].text)
                .then(|| self.longest_mention(text, &words[i..]))
                .flatten();
            match found {
                Some((mention, len)) => {
                    mentions.push(mention);
                    i += len;
                }
                None => i += 1,
            }
        }
        mentions
    }

    fn is_brand(&self, word: &str) -> bool {
        self.all()
            .iter()
            .any(|p| p.name.split(' ').next() == Some(word))
    }

    /// The longest run of words starting at `words[0]` (a brand) that matches at least one
    /// product name, and how many words it spans.
    fn longest_mention(
        &self,
        text: &str,
        words: &[Word<'_>],
    ) -> Option<(ProductMention<'_>, usize)> {
        // The brand, then name-like words; a word ending in punctuation ends the run.
        let mut run = 1;
        while run < words.len() && !words[run - 1].ends_phrase && is_name_word(words[run].text) {
            run += 1;
        }
        (2..=run).rev().find_map(|len| {
            let candidate = candidate(&words[..len]);
            let matches: Vec<&Product> = self
                .all()
                .iter()
                .filter(|p| {
                    p.name == candidate
                        || p.name
                            .strip_prefix(&candidate)
                            .is_some_and(|rest| rest.starts_with(' '))
                })
                .collect();
            let target = match matches.as_slice() {
                [] => return None,
                [one] => MentionTarget::Product(one),
                _ => match matches.iter().find(|p| p.name == candidate) {
                    Some(exact) => MentionTarget::Product(exact),
                    None => MentionTarget::Ambiguous(matches),
                },
            };
            let offset = words[0].offset;
            debug_assert!(text[offset..].starts_with(words[0].text));
            Some((
                ProductMention {
                    text: candidate,
                    offset,
                    target,
                },
                len,
            ))
        })
    }
}

/// A whitespace-separated word with surrounding punctuation removed.
#[derive(Debug, Clone, Copy)]
struct Word<'t> {
    text: &'t str,
    offset: usize,
    /// The word was followed by punctuation (`,`, `.`, `:`, …) that ends a name.
    ends_phrase: bool,
}

fn words(text: &str) -> Vec<Word<'_>> {
    const MARKUP: [char; 4] = ['*', '"', '`', '_'];
    let mut out = Vec::new();
    let mut offset = 0;
    for raw in text.split_inclusive(char::is_whitespace) {
        let start = offset;
        offset += raw.len();
        let raw = raw.trim_end();
        // Leading markup, and an opening parenthesis before a word ("(Breeze"); one before a
        // number stays, it is part of a kit name's "(2x16 GB)".
        let mut word = raw.trim_start_matches(MARKUP);
        if let Some(rest) = word.strip_prefix('(')
            && rest.starts_with(|c: char| c.is_alphabetic())
        {
            word = rest;
        }
        let lead = raw.len() - word.len();
        let core = word
            .trim_end_matches(MARKUP)
            .trim_end_matches([',', '.', ';', ':', '!', '?'])
            .trim_end_matches(MARKUP);
        if core.is_empty() {
            continue;
        }
        out.push(Word {
            text: core,
            offset: start + lead,
            ends_phrase: core.len() < word.len() || core.ends_with(')'),
        });
    }
    out
}

/// `words` joined into a candidate name. A closing parenthesis the candidate did not open is
/// punctuation around it ("(the Breeze Frost 120)"), not part of it.
fn candidate(words: &[Word<'_>]) -> String {
    let mut joined = words.iter().map(|w| w.text).collect::<Vec<_>>().join(" ");
    if joined.matches(')').count() > joined.matches('(').count() {
        joined.pop();
    }
    joined
}

/// Words that continue a product name: capitalised or numeric, or the `(2x16` of a kit name.
fn is_name_word(word: &str) -> bool {
    let word = word.strip_prefix('(').unwrap_or(word);
    word.chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<(String, Vec<String>)> {
        Catalog::global()
            .product_mentions(text)
            .into_iter()
            .map(|m| {
                let ids = match m.target {
                    MentionTarget::Product(p) => vec![p.id.to_string()],
                    MentionTarget::Ambiguous(ps) => ps.iter().map(|p| p.id.to_string()).collect(),
                };
                (m.text, ids)
            })
            .collect()
    }

    #[test]
    fn full_names_and_unique_prefixes_resolve() {
        let found = targets(
            "Pair the Nimbus Stratus DDR5-6000 32 GB (2x16 GB) with a Voltaris V5\n8600, \
             or the Arcwave A4 1 TB.",
        );
        assert_eq!(
            found,
            [
                (
                    "Nimbus Stratus DDR5-6000 32 GB (2x16 GB)".to_owned(),
                    vec!["nimbus-ddr5-6000-32".to_owned()]
                ),
                (
                    "Voltaris V5 8600".to_owned(),
                    vec!["voltaris-v5-8600".to_owned()]
                ),
                (
                    "Arcwave A4 1 TB".to_owned(),
                    vec!["arcwave-a4-1tb".to_owned()]
                ),
            ]
        );
        // A unique prefix is enough.
        assert_eq!(
            targets("the Lumen Apex (52 mm)")[0].1,
            ["lumen-ddr5-7200-32"]
        );
    }

    #[test]
    fn shared_prefixes_are_ambiguous() {
        let found = targets("Can it power the Halcyon HX 7070? The Nimbus Stratus is low.");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "Halcyon HX 7070");
        assert_eq!(found[0].1.len(), 2);
        assert_eq!(found[1].0, "Nimbus Stratus");
        assert!(found[1].1.len() > 2);
    }

    #[test]
    fn brand_references_are_not_mentions() {
        assert!(targets("Helion CPUs need it; Nimbus DDR5 kits are low.").is_empty());
    }

    #[test]
    fn offsets_point_at_the_brand() {
        let text = "Use **Breeze Frost 120** here.";
        let mention = &Catalog::global().product_mentions(text)[0];
        assert!(text[mention.offset..].starts_with("Breeze Frost 120"));
    }
}
