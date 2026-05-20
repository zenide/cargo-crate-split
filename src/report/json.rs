//! JSON renderer — serializes the typed `Analysis` struct.

use anyhow::Result;

use crate::analyze::Analysis;

/// Serialize the analysis to pretty JSON.
pub fn render(a: &Analysis) -> Result<String> {
    Ok(serde_json::to_string_pretty(a)?)
}
