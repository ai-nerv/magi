//! Transient request-frame diagnostics, without prompt content or fingerprints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invalidation {
    ToolsChanged,
    ModelChanged,
    RulesChanged,
    OptionsChanged,
    LayoutReduced,
    LayoutChanged,
    SummaryActivated,
    ResumeIncompatible,
    ObservationUnavailable,
    DialectChanged,
    ProviderPolicyChanged,
    ImagesAdapted,
}
