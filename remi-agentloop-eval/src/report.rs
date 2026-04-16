use remi_core::types::AgentEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageTotals {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub id: String,
    pub name: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationRunResult {
    pub variant_id: String,
    pub variant_label: String,
    pub final_text: String,
    pub reasoning: Option<String>,
    #[serde(default)]
    pub usage: UsageTotals,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResultRecord>,
    #[serde(skip)]
    pub events: Vec<AgentEvent>,
    pub done: bool,
    pub cancelled: bool,
    pub interrupted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationReport {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<EvaluationRunResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreCard {
    pub name: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredRunResult {
    pub run: EvaluationRunResult,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scores: Vec<ScoreCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEvaluationReport {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<ScoredRunResult>,
}