//! `local_models` (local-models.md section 5): the machine and the list with each fit, so
//! "which local model should I use for vision?" has an answer. It never pulls: the owner
//! downloads from Settings -> Local models.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};

use super::LocalModels;

pub type LocalModelsCell = Arc<OnceLock<Arc<LocalModels>>>;

pub struct LocalModelsTool {
    contract: ToolContract,
    cell: LocalModelsCell,
}

impl LocalModelsTool {
    pub fn new(cell: LocalModelsCell) -> Self {
        let contract = ToolContract::read_only(
            "local_models",
            "What this machine can run locally and what KAgent recommends: the graphics card and its free memory, whether Ollama is running, the models installed, and the shipped list of recommended local models per job (chat, vision, embedding, speech to text) with a fit verdict (fits, tight, too big, slow). Use it for 'which local model should I use for vision?'. It never downloads: say the owner can press Download in Settings -> Local models.",
            json!({"type":"object","properties":{}}),
        );
        Self { contract, cell }
    }
}

/// The answer as text for the model, from the view of `GET /api/local-models`.
pub fn describe(view: &Value) -> String {
    let mut out = Vec::new();
    let o = &view["ollama"];
    if o["reachable"] == true {
        out.push(format!("Ollama {} is running at {}.", o["version"].as_str().unwrap_or("(version unknown)"), o["baseUrl"].as_str().unwrap_or("")));
    } else {
        out.push(format!("Ollama is not reachable at {}: the owner starts it (KAgent does not install it). Nothing can be downloaded until then.", o["baseUrl"].as_str().unwrap_or("")));
    }
    match view["gpu"].as_object() {
        Some(g) => out.push(format!(
            "Graphics card: {}, {} MB of memory, {} MB free now. RAM {} MB.",
            g["name"].as_str().unwrap_or("?"),
            g["vramTotalMb"],
            g["vramFreeMb"],
            view["ramTotalMb"]
        )),
        None => out.push(format!("No graphics card found: models run on the processor and are slow. RAM {} MB.", view["ramTotalMb"])),
    }
    for job in ["chat", "vision", "embedding", "stt"] {
        let mut lines = Vec::new();
        for m in view["recommended"].as_array().into_iter().flatten().filter(|m| m["jobs"].as_array().is_some_and(|j| j.iter().any(|x| x == job))) {
            let mut l = format!("- {} ({}), {} MB, fit: {}", m["name"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""), m["sizeMb"], m["fit"].as_str().unwrap_or(""));
            if m["recommendedFor"].as_array().is_some_and(|r| r.iter().any(|x| x == job)) {
                l.push_str(", RECOMMENDED");
            }
            if m["installed"] == true {
                l.push_str(", installed");
            }
            if m["tools"] == false && job == "chat" {
                l.push_str(", no tool calls (a helper, not the Brain)");
            }
            if m["downloadable"] == false {
                l.push_str(", not usable yet");
            }
            lines.push(l);
        }
        if !lines.is_empty() {
            out.push(format!("{job}:\n{}", lines.join("\n")));
        }
    }
    out.push("To install one the owner presses Download in Settings -> Local models; you cannot download it.".into());
    out.join("\n")
}

#[async_trait]
impl Tool for LocalModelsTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, _input: &Value) -> CallAssessment {
        CallAssessment::from_contract(&self.contract, "list the local models this machine can run".to_string(), "local_models".to_string())
    }

    async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Some(lm) = self.cell.get() else { return Ok(ToolOutput::error("the local models module is not running")) };
        Ok(ToolOutput::ok(describe(&lm.view().await)))
    }
}
