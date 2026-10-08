//! The same STDIO MCP server using optional SDK derives and rmcp's ToolRouter.
//! Run with `cargo run --example derived --features derive`; the host drives it.

use openai_mcp_extensions::{Model, SettingsModel, StringEnum};
use serde::{Deserialize, Serialize};

#[path = "support/typed_server.rs"]
mod typed_server;

const USE_ROUTER: bool = true;

#[derive(Clone, Serialize, Deserialize, StringEnum)]
#[serde(rename_all = "lowercase")]
enum Units {
    Mm,
    In,
}

#[derive(Deserialize, Model)]
#[serde(rename_all = "camelCase")]
struct Answer {
    #[openai(title = "Part code", pattern = "^[A-Z]{2}-[0-9]{3}$")]
    part_code: String,
    // Parts are packaged in pairs, so the quantity must be even.
    #[openai(title = "Quantity", minimum = 2, maximum = 100, multiple_of = 2)]
    quantity: u8,
    #[openai(title = "Measurement units")]
    units: Units,
    #[openai(title = "Optional note")]
    note: Option<String>,
}

// Complete values only need Serialize. SettingsModel generates a Deserialize
// PreferencesPatch with Option fields; the SDK does not serialize that patch.
#[derive(Clone, Serialize, SettingsModel)]
#[serde(rename_all = "camelCase")]
struct Preferences {
    #[openai(title = "Show grid")]
    show_grid: bool,
    #[openai(title = "Measurement units")]
    units: Units,
    // Zero means no preferred quantity; other defaults are pairs.
    #[openai(title = "Default quantity", maximum = 100, multiple_of = 2)]
    default_quantity: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    typed_server::run(USE_ROUTER).await
}
