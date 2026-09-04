use anyhow::{Context as _, Result};
use macro_sync::CollaborativeDocument;

const FIXTURE: &str = include_str!("../fixtures/markdown_two_client_typescript.json");

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(hex.len().is_multiple_of(2), "fixture hex has half a byte");
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair)?;
            u8::from_str_radix(pair, 16).context("fixture contains non-hexadecimal bytes")
        })
        .collect()
}

fn main() -> Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE)?;
    let snapshot = decode_hex(
        fixture["seed_snapshot_hex"]
            .as_str()
            .context("fixture has no seed snapshot")?,
    )?;
    let document = CollaborativeDocument::from_snapshot(&snapshot, Some(702))?;
    let baseline = document.encoded_version_vector();
    let mut state = document.deep_json()?;
    state["root"]["children"][1]["children"][0]["text"] = "Browser heading from native".into();
    document.replace_markdown_state(&state)?;
    let update = document.export_updates_since(&baseline)?;
    let update_hex = update
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!("{update_hex}");
    Ok(())
}
