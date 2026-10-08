//! A STDIO MCP server with handwritten models and a custom tool registry.
//! Run with `cargo run --example typed --no-default-features`; the host drives it.

use openai_mcp_extensions::{Error, FieldType, Model, ModelField, settings::SettingsModel};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "support/typed_server.rs"]
mod typed_server;

const USE_ROUTER: bool = false;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Units {
    Mm,
    In,
}

impl FieldType for Units {
    fn schema() -> Result<Value, Error> {
        Ok(json!({"type":"string","enum":["mm","in"]}))
    }
}

// Only accepted form content is deserialized; the shared server builds its reply.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    part_code: String,
    quantity: u8,
    units: Units,
    note: Option<String>,
}

impl Model for Answer {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<String>("partCode", "Part code")?.with_annotations(
                json!({"pattern":"^[A-Z]{2}-[0-9]{3}$"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )?,
            // Parts are packaged in pairs, so the quantity must be even.
            ModelField::new::<u8>("quantity", "Quantity")?.with_annotations(
                json!({"minimum":2,"maximum":100,"multipleOf":2})
                    .as_object()
                    .unwrap()
                    .clone(),
            )?,
            ModelField::new::<Units>("units", "Measurement units")?,
            ModelField::new::<Option<String>>("note", "Optional note")?,
        ])
    }
}

// The SDK serializes complete values and deserializes patches, never the reverse.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    show_grid: bool,
    units: Units,
    default_quantity: u8,
}

impl Model for Preferences {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<bool>("showGrid", "Show grid")?,
            ModelField::new::<Units>("units", "Measurement units")?,
            // Zero means no preferred quantity; other defaults are pairs.
            ModelField::new::<u8>("defaultQuantity", "Default quantity")?.with_annotations(
                json!({"maximum":100,"multipleOf":2})
                    .as_object()
                    .unwrap()
                    .clone(),
            )?,
        ])
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferencesPatch {
    show_grid: Option<bool>,
    units: Option<Units>,
    default_quantity: Option<u8>,
}

impl Model for PreferencesPatch {
    fn fields() -> Result<Vec<ModelField>, Error> {
        Ok(vec![
            ModelField::new::<Option<bool>>("showGrid", "Show grid")?,
            ModelField::new::<Option<Units>>("units", "Measurement units")?,
            ModelField::new::<Option<u8>>("defaultQuantity", "Default quantity")?
                .with_annotations(
                    json!({"maximum":100,"multipleOf":2})
                        .as_object()
                        .unwrap()
                        .clone(),
                )?,
        ])
    }
}

impl SettingsModel for Preferences {
    type Patch = PreferencesPatch;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    typed_server::run(USE_ROUTER).await
}
