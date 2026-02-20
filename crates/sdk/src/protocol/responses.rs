use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! opaque_struct {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            #[serde(flatten)]
            pub extra: serde_json::Map<String, Value>,
        }
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_error_info: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_details: Option<Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub items: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResult {
    pub thread: ThreadSummary,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub turn: Turn,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResult {
    pub turn_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResult {
    #[serde(default)]
    pub data: Vec<ThreadSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLoadedListResult {
    #[serde(default)]
    pub data: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_personality: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResult {
    #[serde(default)]
    pub data: Vec<ModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

opaque_struct!(ExperimentalFeatureListResult);
opaque_struct!(SkillsListResult);
opaque_struct!(SkillsRemoteReadResult);
opaque_struct!(SkillsRemoteWriteResult);
opaque_struct!(SkillsConfigWriteResult);
opaque_struct!(AppsListResult);
opaque_struct!(ReviewStartResult);
opaque_struct!(McpServerOauthLoginResult);
opaque_struct!(McpServerStatusListResult);
opaque_struct!(FeedbackUploadResult);
opaque_struct!(CommandExecResult);
opaque_struct!(ConfigReadResult);
opaque_struct!(ConfigValueWriteResult);
opaque_struct!(ConfigBatchWriteResult);
opaque_struct!(ConfigRequirementsReadResult);
opaque_struct!(LoginAccountResult);
opaque_struct!(GetAccountResult);
opaque_struct!(AccountRateLimitsReadResult);
opaque_struct!(ThreadArchiveResult);
opaque_struct!(ThreadUnarchiveResult);
opaque_struct!(ThreadCompactStartResult);
opaque_struct!(ThreadSetNameResult);
opaque_struct!(ThreadRollbackResult);
opaque_struct!(ThreadReadResult);
