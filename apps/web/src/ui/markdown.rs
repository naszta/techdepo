//! A tiny Markdown renderer for catalog content: paragraphs, `- ` lists, `**bold**`, `*em*`
//! and `` `code` ``. Catalog content is generated and validated, so this is all it uses.

use dioxus::prelude::*;

/// Renders Markdown blocks as prose.
#[component]
pub fn Markdown(text: String, #[props(default)] class: String) -> Element {
    let blocks = blocks(&text);
    rsx! {
        div { class: "flex flex-col gap-3 leading-relaxed {class}",
            for block in blocks {
                match block {
                    Block::Paragraph(text) => rsx! { p { {inline(&text)} } },
                    Block::List(items) => rsx! {
                        ul { class: "list-disc pl-5 flex flex-col gap-1",
                            for item in items {
                                li { {inline(&item)} }
                            }
                        }
                    },
                }
            }
        }
    }
}

enum Block {
    Paragraph(String),
    List(Vec<String>),
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    for chunk in text.split("\n\n") {
        let lines: Vec<&str> = chunk
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        if lines.iter().all(|l| l.starts_with("- ")) {
            out.push(Block::List(
                lines.iter().map(|l| l[2..].to_owned()).collect(),
            ));
        } else {
            out.push(Block::Paragraph(lines.join(" ")));
        }
    }
    out
}

/// Inline emphasis and code spans.
fn inline(text: &str) -> Element {
    let mut parts: Vec<(u8, String)> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let next = [("**", 1u8), ("`", 3u8), ("*", 2u8)]
            .into_iter()
            .filter_map(|(marker, kind)| rest.find(marker).map(|at| (at, marker, kind)))
            .min_by_key(|(at, marker, _)| (*at, std::cmp::Reverse(marker.len())));
        let Some((at, marker, kind)) = next else {
            parts.push((0, rest.to_owned()));
            break;
        };
        let after = &rest[at + marker.len()..];
        match after.find(marker) {
            Some(end) if end > 0 => {
                parts.push((0, rest[..at].to_owned()));
                parts.push((kind, after[..end].to_owned()));
                rest = &after[end + marker.len()..];
            }
            _ => {
                parts.push((0, rest[..at + marker.len()].to_owned()));
                rest = after;
            }
        }
    }
    rsx! {
        for (kind , text) in parts {
            match kind {
                1 => rsx! { strong { class: "font-semibold", "{text}" } },
                2 => rsx! { em { "{text}" } },
                3 => rsx! { code { class: "font-mono text-[0.9em] bg-base-200 rounded px-1", "{text}" } },
                _ => rsx! { "{text}" },
            }
        }
    }
}
