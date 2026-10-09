use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAttachment {
    pub provider: String,
    pub home_id: String,
    pub session_id: String,
    pub location: String,
    pub host_id: String,
    pub attach_argv: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeBinding {
    pub pane_id: String,
    pub binding_id: String,
    pub attachment: RuntimeAttachment,
    pub seq: Option<u64>,
    pub state: RuntimeState,
    pub label: Option<String>,
    pub ttl_ms: u64,
    pub fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneBindRuntimeParams {
    pub pane_id: String,
    pub binding_id: String,
    pub attachment: RuntimeAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneReportRuntimeParams {
    pub pane_id: String,
    pub binding_id: String,
    pub seq: u64,
    pub state: RuntimeState,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneUnbindRuntimeParams {
    pub pane_id: String,
    pub binding_id: String,
}
