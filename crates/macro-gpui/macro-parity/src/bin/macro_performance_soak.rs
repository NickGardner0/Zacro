use anyhow::{Context as _, Result, ensure};
use macro_api::{
    AgentLogDirection, AgentSessionBot, AgentSessionLogEntry, AgentSessionLogResponse,
    fold_agent_session_log,
};
use macro_canvas::{Canvas, CanvasModel, CanvasNode, Point};
use macro_editor::{
    DocumentSearchOptions, document_analysis, document_search_matches, markdown_to_lexical_state,
};
use macro_list::{InboxTab, ListInteraction};
use macro_realtime::parse_comms_realtime_event;
use serde_json::{Value, json};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

const SCHEMA: &str = "macro.gpui.performance-soak.v1";

struct SoakCase {
    name: &'static str,
    items: usize,
    budget_ms: u128,
    elapsed_ms: u128,
}

fn main() -> Result<()> {
    let started = Instant::now();
    let cases = vec![
        run_case("inbox_classification", 100_000, 1_500, soak_inbox)?,
        run_case("channel_realtime_decode", 50_000, 2_500, soak_channels)?,
        run_case("document_projection", 25_000, 3_000, soak_document)?,
        run_case("crm_table_interaction", 100_000, 1_500, soak_crm_table)?,
        run_case("canvas_geometry", 10_000, 3_000, soak_canvas)?,
        run_case("agent_transcript_fold", 2_000, 3_000, soak_agent_transcript)?,
    ];
    let total_elapsed_ms = started.elapsed().as_millis();
    ensure!(
        total_elapsed_ms <= 12_000,
        "performance soak exceeded the 12000 ms aggregate budget: {total_elapsed_ms} ms"
    );

    let report = json!({
        "schema": SCHEMA,
        "profile": if cfg!(debug_assertions) { "optimized-dev" } else { "release" },
        "aggregate_budget_ms": 12_000,
        "aggregate_elapsed_ms": total_elapsed_ms,
        "cases": cases.iter().map(|case| json!({
            "name": case.name,
            "items": case.items,
            "budget_ms": case.budget_ms,
            "elapsed_ms": case.elapsed_ms,
            "passed": case.elapsed_ms <= case.budget_ms,
        })).collect::<Vec<_>>(),
    });
    let output_path = std::env::var_os("MACRO_SOAK_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/macro-gpui/performance-soak.json"));
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&output_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_case(
    name: &'static str,
    items: usize,
    budget_ms: u128,
    operation: fn(usize) -> Result<()>,
) -> Result<SoakCase> {
    let started = Instant::now();
    operation(items).with_context(|| format!("{name} soak failed"))?;
    let elapsed_ms = started.elapsed().as_millis();
    ensure!(
        elapsed_ms <= budget_ms,
        "{name} exceeded its {budget_ms} ms budget: {elapsed_ms} ms"
    );
    Ok(SoakCase {
        name,
        items,
        budget_ms,
        elapsed_ms,
    })
}

fn soak_inbox(items: usize) -> Result<()> {
    let mut signal = 0_usize;
    let mut noise = 0_usize;
    for index in 0..items {
        let label = if index % 5 == 0 {
            "CATEGORY_PROMOTIONS"
        } else {
            "CATEGORY_PERSONAL"
        };
        let raw = json!({
            "data": {
                "id": format!("email-{index}"),
                "labels": [{"providerLabelId": label}],
                "subject": format!("Synthetic message {index}"),
            }
        });
        if InboxTab::Signal.matches("emailThread", &raw) {
            signal += 1;
        } else {
            noise += 1;
        }
    }
    ensure!(signal == items - items.div_ceil(5));
    ensure!(noise == items.div_ceil(5));
    black_box((signal, noise));
    Ok(())
}

fn soak_channels(items: usize) -> Result<()> {
    let mut decoded = 0_usize;
    for index in 0..items {
        let raw = json!({
            "channelId": format!("channel-{}", index % 64),
            "id": format!("message-{index}"),
            "threadId": (index % 7 == 0).then(|| format!("thread-{}", index / 7)),
            "content": format!("Realtime message {index}"),
            "createdAt": "2026-08-31T12:00:00Z",
        });
        decoded += parse_comms_realtime_event("comms_message", &raw).is_some() as usize;
    }
    ensure!(decoded == items);
    black_box(decoded);
    Ok(())
}

fn soak_document(items: usize) -> Result<()> {
    let mut markdown = String::with_capacity(items * 96);
    for index in 0..items {
        if index % 50 == 0 {
            markdown.push_str(&format!("## Section {index}\n\n"));
        }
        markdown.push_str(&format!(
            "Block {index} keeps native projection, search, and collaboration bounded.\n\n"
        ));
    }
    let analysis = document_analysis(&markdown);
    let lexical = markdown_to_lexical_state(&markdown);
    let matches =
        document_search_matches(&markdown, "collaboration", DocumentSearchOptions::default())
            .map_err(anyhow::Error::msg)?;
    ensure!(analysis.stats.lines >= items);
    ensure!(analysis.blocks.len() >= items);
    ensure!(matches.len() == items);
    ensure!(lexical.get("root").is_some());
    black_box((analysis, lexical, matches));
    Ok(())
}

fn soak_crm_table(items: usize) -> Result<()> {
    let ids = (0..items)
        .map(|index| format!("crm-company-{index}"))
        .collect::<Vec<_>>();
    let mut interaction = ListInteraction::default();
    for index in 0..items {
        interaction.set_focused(index, ids.len());
        if index % 10 == 0 {
            interaction.toggle_selected(&ids);
        }
    }
    ensure!(interaction.focused() == Some(items - 1));
    ensure!(interaction.selected_count() == items.div_ceil(10));
    black_box((ids, interaction));
    Ok(())
}

fn soak_canvas(items: usize) -> Result<()> {
    let nodes = (0..items)
        .map(|index| {
            let mut node = CanvasNode::text(
                format!("Canvas node {index}"),
                (index % 100) as f64 * 220.0,
                (index / 100) as f64 * 72.0,
            );
            node.id = format!("node-{index}");
            node
        })
        .collect();
    let canvas = Canvas {
        nodes,
        ..Default::default()
    };
    let encoded = canvas.to_json()?;
    let decoded = Canvas::from_json(&encoded)?;
    let mut model = CanvasModel::new(decoded)?;
    model.select_intersecting_rect(
        Point { x: 0.0, y: 0.0 },
        Point {
            x: 22_000.0,
            y: 7_200.0,
        },
        false,
    );
    ensure!(model.canvas().nodes.len() == items);
    ensure!(model.content_bounds().is_some());
    ensure!(!model.selected_nodes().is_empty());
    black_box((model, encoded));
    Ok(())
}

fn soak_agent_transcript(items: usize) -> Result<()> {
    let mut entries = Vec::with_capacity(items * 3);
    for index in 0..items {
        let request_id = format!("agent_session:prompt-{index}");
        entries.push(log_entry(
            AgentLogDirection::ToRuntime,
            json!({
                "type": "acp",
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "session/prompt",
                "params": {
                    "sessionId": "soak-session",
                    "prompt": [{"type": "text", "text": format!("Prompt {index}")}]
                }
            }),
            index * 3,
        ));
        entries.push(log_entry(
            AgentLogDirection::ToServer,
            json!({
                "type": "acp",
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "soak-session",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": format!("Response {index}")}
                    }
                }
            }),
            index * 3 + 1,
        ));
        entries.push(log_entry(
            AgentLogDirection::ToServer,
            json!({
                "type": "acp",
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"stopReason": "end_turn"}
            }),
            index * 3 + 2,
        ));
    }
    let transcript = fold_agent_session_log(
        "soak-session",
        &AgentSessionLogResponse {
            bot: AgentSessionBot {
                avatar_url: None,
                id: "macro".into(),
                name: "Macro".into(),
            },
            entries,
        },
    )?;
    ensure!(transcript.messages.len() == items * 2);
    black_box(transcript);
    Ok(())
}

fn log_entry(direction: AgentLogDirection, content: Value, index: usize) -> AgentSessionLogEntry {
    AgentSessionLogEntry {
        content,
        direction,
        created_at: format!(
            "2026-08-31T12:{:02}:{:02}.{:03}Z",
            index / 60 % 60,
            index % 60,
            index
        ),
        user_id: (direction == AgentLogDirection::ToRuntime).then(|| "macro|soak".into()),
    }
}
