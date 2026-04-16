pub mod capture;
pub mod error;
pub mod report;
pub mod runner;
pub mod variant;

pub use capture::SessionCapture;
pub use error::EvalError;
pub use report::{
    EvaluationReport, EvaluationRunResult, ScoreCard, ScoredEvaluationReport, ScoredRunResult,
    ToolResultRecord, UsageTotals,
};
pub use runner::{build_replay_input, ExperimentRunner, Scorer};
pub use variant::{ExperimentVariant, ToolOverrideMode};

pub mod prelude {
    pub use crate::capture::SessionCapture;
    pub use crate::error::EvalError;
    pub use crate::report::{
        EvaluationReport, EvaluationRunResult, ScoreCard, ScoredEvaluationReport,
        ScoredRunResult, ToolResultRecord, UsageTotals,
    };
    pub use crate::runner::{build_replay_input, ExperimentRunner, Scorer};
    pub use crate::variant::{ExperimentVariant, ToolOverrideMode};
}