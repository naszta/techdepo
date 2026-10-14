//! Buying guides and policies from the compiled-in knowledge base ([`crate::agents::kb`]): list
//! them (slug, title, summary), then read the one that fits ("agentic retrieval", support.md §7).

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use techdepo_agent::{Runtime, Tool, ToolError};

use super::ToolCx;
use super::account::NoArgs;
use crate::agents::kb::{self, DocKind};

/// Arguments of `read_guide` / `read_policy`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SlugArgs {
    /// The document's `slug` from the list.
    pub slug: String,
}

fn list(kind: DocKind) -> Value {
    let docs: Vec<Value> = kb::docs(kind)
        .map(|d| json!({ "slug": d.slug, "title": d.title, "summary": d.summary }))
        .collect();
    json!({ "documents": docs })
}

fn read(kind: DocKind, slug: &str, list_tool: &str) -> Result<Value, ToolError> {
    let doc = kb::get(kind, slug).ok_or_else(|| {
        ToolError::NotFound(format!(
            "no document `{slug}`; call {list_tool} for the slugs"
        ))
    })?;
    Ok(json!({ "slug": doc.slug, "title": doc.title, "content": doc.body }))
}

/// `list_guides`.
pub struct ListGuides;

impl<R: Runtime> Tool<ToolCx<'_, R>> for ListGuides {
    const NAME: &'static str = "list_guides";
    const DESCRIPTION: &'static str = "The shop's buying guides (slug, title, summary): DDR4 vs \
        DDR5, PSU sizing, coolers, form factors and clearance, BIOS updates, storage, GPU power.";
    type Args = NoArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, _args: NoArgs) -> Result<Value, ToolError> {
        Ok(list(DocKind::Guide))
    }
}

/// `read_guide`.
pub struct ReadGuide;

impl<R: Runtime> Tool<ToolCx<'_, R>> for ReadGuide {
    const NAME: &'static str = "read_guide";
    const DESCRIPTION: &'static str = "One buying guide's full text.";
    type Args = SlugArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: SlugArgs) -> Result<Value, ToolError> {
        read(DocKind::Guide, &args.slug, "list_guides")
    }
}

/// `list_policies`.
pub struct ListPolicies;

impl<R: Runtime> Tool<ToolCx<'_, R>> for ListPolicies {
    const NAME: &'static str = "list_policies";
    const DESCRIPTION: &'static str = "The shop's policies (slug, title, summary): shipping, \
        returns, refunds, cancellations, payment, invoices and VAT, warranty, contacting \
        support.";
    type Args = NoArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, _args: NoArgs) -> Result<Value, ToolError> {
        Ok(list(DocKind::Policy))
    }
}

/// `read_policy`.
pub struct ReadPolicy;

impl<R: Runtime> Tool<ToolCx<'_, R>> for ReadPolicy {
    const NAME: &'static str = "read_policy";
    const DESCRIPTION: &'static str = "One policy's full text.";
    type Args = SlugArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: SlugArgs) -> Result<Value, ToolError> {
        read(DocKind::Policy, &args.slug, "list_policies")
    }
}
