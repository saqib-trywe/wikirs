//! The Operation trait and the erased registry every adapter is generated from (ADR 0004).

use std::panic::{AssertUnwindSafe, catch_unwind};

use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{Error, Result, Wiki, plan::Warning};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Query,
    Mutation,
    Maintenance,
    Subscription,
}

pub trait Operation {
    const NAME: &'static str;
    const KIND: Kind;
    const DESCRIPTION: &'static str;
    type Input: DeserializeOwned + JsonSchema;
    type Output: Serialize + JsonSchema;

    fn run(wiki: &Wiki, input: Self::Input) -> Result<Self::Output>;

    /// Warnings carried next to the result in the `{ result, warnings }` envelope.
    fn warnings(_output: &Self::Output) -> Vec<Warning> {
        Vec::new()
    }
}

/// Runs a typed Operation and wraps it as `{ result, warnings }`, turning a panic
/// into `internal` so one bug can't take a long-lived process down.
pub fn run_enveloped<O: Operation>(wiki: &Wiki, input: O::Input) -> Result<Value> {
    let output = catch_unwind(AssertUnwindSafe(|| O::run(wiki, input)))
        .map_err(|_| Error::internal(format!("`{}` panicked", O::NAME)))??;
    let warnings = O::warnings(&output);
    let result = serde_json::to_value(&output).map_err(|e| Error::internal(e.to_string()))?;
    Ok(json!({ "result": result, "warnings": warnings }))
}

/// One registry entry: everything an adapter needs, with no knowledge of the type.
#[derive(Clone, Copy)]
pub struct OpInfo {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
    input_schema: fn() -> Value,
    output_schema: fn() -> Value,
    call: fn(&Wiki, Value) -> Result<Value>,
}

impl OpInfo {
    #[must_use]
    pub fn of<O: Operation>() -> Self {
        Self {
            name: O::NAME,
            kind: O::KIND,
            description: O::DESCRIPTION,
            input_schema: || schema_value::<O::Input>(),
            output_schema: || schema_value::<O::Output>(),
            call: |wiki, input| {
                let input: O::Input = serde_json::from_value(input)
                    .map_err(|e| Error::invalid_input(None, e.to_string()))?;
                run_enveloped::<O>(wiki, input)
            },
        }
    }

    #[must_use]
    pub fn input_schema(&self) -> Value {
        (self.input_schema)()
    }

    #[must_use]
    pub fn output_schema(&self) -> Value {
        (self.output_schema)()
    }

    /// JSON in, `{ result, warnings }` out.
    pub fn call(&self, wiki: &Wiki, input: Value) -> Result<Value> {
        (self.call)(wiki, input)
    }
}

fn schema_value<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schemas serialize")
}

/// Looks an Operation up by its catalogue name.
#[must_use]
pub fn find(name: &str) -> Option<OpInfo> {
    crate::ops::registry()
        .into_iter()
        .find(|op| op.name == name)
}

/// The catalogue as JSON: names, kinds, descriptions, schemas, binary version.
#[must_use]
pub fn catalogue() -> Value {
    let ops: Vec<Value> = crate::ops::registry()
        .iter()
        .map(|op| {
            json!({
                "name": op.name,
                "kind": op.kind,
                "description": op.description,
                "input_schema": op.input_schema(),
                "output_schema": op.output_schema(),
            })
        })
        .collect();
    json!({ "version": env!("CARGO_PKG_VERSION"), "operations": ops })
}

/// Declares every Operation exactly once. Generates `registry()` and, with the
/// `clap` feature, the CLI's `Command` enum plus its dispatch.
#[macro_export]
macro_rules! operations {
    ($($op:ident),* $(,)?) => {
        /// Every Operation in the catalogue, in declaration order.
        #[must_use]
        pub fn registry() -> ::std::vec::Vec<$crate::OpInfo> {
            ::std::vec![$($crate::OpInfo::of::<$op>()),*]
        }

        /// One CLI subcommand per Operation (kebab-cased catalogue name).
        #[cfg(feature = "clap")]
        #[derive(Debug, ::clap::Subcommand)]
        pub enum Command {
            $($op(<$op as $crate::Operation>::Input)),*
        }

        #[cfg(feature = "clap")]
        impl Command {
            /// Runs the chosen Operation, returning the `{ result, warnings }` envelope.
            pub fn run(self, wiki: &$crate::Wiki) -> $crate::Result<::serde_json::Value> {
                match self {
                    $(Self::$op(input) => $crate::run_enveloped::<$op>(wiki, input)),*
                }
            }
        }
    };
}
