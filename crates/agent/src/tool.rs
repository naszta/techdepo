//! Tools: typed arguments in, JSON out, schema sent to the model.

use std::future::Future;
use std::pin::Pin;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;
use techdepo_ports::ToolDefinition;

/// A tool failed. Every kind but [`ToolError::Fatal`] goes back to the model as the tool's
/// result, so it can correct itself (fix the arguments, search again, ask the customer).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    /// The arguments did not match the schema, or are semantically wrong.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    /// What the arguments point at does not exist (or is not the customer's).
    #[error("not found: {0}")]
    NotFound(String),
    /// Anything else the model should hear about.
    #[error("{0}")]
    Failed(String),
    /// The tool cannot work at all (its journaled step failed for good): the run ends with
    /// an escalation.
    #[error("{0}")]
    Fatal(String),
}

/// A tool the model can call.
///
/// `Cx` is the caller's per-run context: in the services a struct holding the Restate context
/// and the tenant scope, so a tool never takes the org or customer from its arguments.
pub trait Tool<Cx: ?Sized>: Send + Sync {
    /// Name the model calls it by, e.g. `check_compatibility`.
    const NAME: &'static str;
    /// What it does and when to use it (sent to the model).
    const DESCRIPTION: &'static str;
    /// Arguments; their JSON Schema (schemars) is sent to the model. Doc comments on the fields
    /// become property descriptions.
    type Args: DeserializeOwned + JsonSchema + Send;

    /// Runs the tool. The result goes back to the model as JSON.
    fn call(
        &self,
        cx: &Cx,
        args: Self::Args,
    ) -> impl Future<Output = Result<Value, ToolError>> + Send;
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// [`Tool`] with the argument type erased, so a [`ToolSet`] can hold different tools.
trait ErasedTool<Cx: ?Sized>: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolDefinition;
    fn call<'a>(&'a self, cx: &'a Cx, args: Value) -> BoxFuture<'a, Result<Value, ToolError>>;
}

impl<Cx, T> ErasedTool<Cx> for T
where
    Cx: ?Sized + Sync,
    T: Tool<Cx>,
{
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: T::NAME.to_owned(),
            description: T::DESCRIPTION.to_owned(),
            parameters: schema_for::<T::Args>(),
        }
    }

    fn call<'a>(&'a self, cx: &'a Cx, args: Value) -> BoxFuture<'a, Result<Value, ToolError>> {
        Box::pin(async move {
            let args = parse_args::<T::Args>(args)?;
            Tool::call(self, cx, args).await
        })
    }
}

/// Decodes tool arguments. Models sometimes send them JSON-encoded in a string, or `null` for
/// a tool without parameters; both are accepted.
pub(crate) fn parse_args<A: DeserializeOwned>(args: Value) -> Result<A, ToolError> {
    let args = match args {
        Value::Null => Value::Object(serde_json::Map::new()),
        Value::String(text) => serde_json::from_str(&text)
            .map_err(|e| ToolError::InvalidArguments(format!("arguments are not JSON: {e}")))?,
        other => other,
    };
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

/// The JSON Schema the model sees for `T`: subschemas inlined (small models handle `$ref`
/// poorly), no `$schema` or `title`.
pub fn schema_for<T: JsonSchema + ?Sized>() -> Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let mut value = schema.to_value();
    if let Value::Object(map) = &mut value {
        map.remove("title");
        map.entry("type").or_insert_with(|| "object".into());
        if map.get("type").and_then(Value::as_str) == Some("object") {
            map.entry("properties")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
        }
    }
    value
}

/// The tools of one agent, by name.
pub struct ToolSet<Cx: ?Sized> {
    tools: Vec<Box<dyn ErasedTool<Cx>>>,
}

impl<Cx: ?Sized + Sync> Default for ToolSet<Cx> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Cx: ?Sized> std::fmt::Debug for ToolSet<Cx> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.tools.iter().map(|t| t.name()))
            .finish()
    }
}

impl<Cx: ?Sized + Sync> ToolSet<Cx> {
    /// No tools.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Adds `tool`.
    ///
    /// # Panics
    ///
    /// If a tool of that name is already in the set, or the name is a terminal tool's: both
    /// are programming errors in the agent's definition.
    #[must_use]
    pub fn with<T: Tool<Cx> + 'static>(mut self, tool: T) -> Self {
        assert!(
            self.get(T::NAME).is_none() && !crate::terminal::is_terminal(T::NAME),
            "duplicate tool name `{}`",
            T::NAME
        );
        self.tools.push(Box::new(tool));
        self
    }

    /// Tool names, in the order added.
    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// Whether the set has no tools.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The definitions sent to the model, in the order added.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    fn get(&self, name: &str) -> Option<&dyn ErasedTool<Cx>> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Runs tool `name` with `args`. An unknown name is an error for the model, like bad
    /// arguments.
    pub async fn dispatch(&self, cx: &Cx, name: &str, args: Value) -> Result<Value, ToolError> {
        match self.get(name) {
            Some(tool) => tool.call(cx, args).await,
            None => Err(ToolError::InvalidArguments(format!(
                "there is no tool named `{name}`; available: {}",
                self.names().join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    struct Scope {
        customer: &'static str,
    }

    /// Arguments of `get_order`.
    #[derive(Deserialize, JsonSchema)]
    struct GetOrderArgs {
        /// Order number, e.g. `TD-1042`.
        order_number: String,
        #[serde(default)]
        verbose: Option<bool>,
    }

    struct GetOrder;

    impl Tool<Scope> for GetOrder {
        const NAME: &'static str = "get_order";
        const DESCRIPTION: &'static str = "Looks up one of the customer's orders.";
        type Args = GetOrderArgs;

        async fn call(&self, cx: &Scope, args: GetOrderArgs) -> Result<Value, ToolError> {
            if args.order_number == "TD-9" {
                return Err(ToolError::NotFound("no such order".into()));
            }
            Ok(
                json!({ "number": args.order_number, "customer": cx.customer, "verbose": args.verbose }),
            )
        }
    }

    #[test]
    fn dispatches_by_name_and_reports_bad_calls() {
        let tools = ToolSet::<Scope>::new().with(GetOrder);
        let cx = Scope { customer: "cus_1" };

        let ok = block_on(tools.dispatch(&cx, "get_order", json!({ "order_number": "TD-1" })));
        assert_eq!(ok.unwrap()["customer"], "cus_1");

        let stringly = block_on(tools.dispatch(
            &cx,
            "get_order",
            Value::String(r#"{"order_number":"TD-2"}"#.into()),
        ));
        assert_eq!(stringly.unwrap()["number"], "TD-2");

        let bad = block_on(tools.dispatch(&cx, "get_order", json!({ "number": 1 })));
        assert!(matches!(bad, Err(ToolError::InvalidArguments(_))));

        let missing = block_on(tools.dispatch(&cx, "get_order", json!({ "order_number": "TD-9" })));
        assert_eq!(missing, Err(ToolError::NotFound("no such order".into())));

        let unknown = block_on(tools.dispatch(&cx, "drop_tables", json!({})));
        assert!(matches!(unknown, Err(ToolError::InvalidArguments(m)) if m.contains("get_order")));
    }

    #[test]
    fn exports_inline_schemas() {
        let tools = ToolSet::<Scope>::new().with(GetOrder);
        let defs = tools.definitions();
        assert_eq!(defs.len(), 1);
        let params = &defs[0].parameters;
        assert_eq!(params["type"], "object");
        assert_eq!(
            params["properties"]["order_number"]["description"],
            "Order number, e.g. `TD-1042`."
        );
        assert_eq!(params["required"], json!(["order_number"]));
        assert!(params.get("$schema").is_none() && params.get("title").is_none());

        #[derive(Deserialize, JsonSchema)]
        struct NoArgs {}
        assert_eq!(
            schema_for::<NoArgs>()["properties"],
            json!({}),
            "parameterless tools still declare an object"
        );
    }

    #[test]
    #[should_panic(expected = "duplicate tool name")]
    fn rejects_duplicates() {
        let _ = ToolSet::<Scope>::new().with(GetOrder).with(GetOrder);
    }
}
