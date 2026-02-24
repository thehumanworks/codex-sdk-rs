use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::responses::{ThreadSummary, Turn};

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
pub struct ErrorNotification {
    pub error: super::responses::TurnError,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartedNotification {
    pub thread: ThreadSummary,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadNameUpdatedNotification {
    pub thread_id: String,
    pub name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsageUpdatedNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub turn: Turn,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub turn: Turn,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiffUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub step: String,
    pub status: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedNotification {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(default)]
    pub plan: Vec<PlanStep>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ItemLifecycleNotification {
    #[serde(default)]
    pub item: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeltaNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_index: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

opaque_struct!(McpServerOauthLoginCompletedNotification);
opaque_struct!(AccountUpdatedNotification);
opaque_struct!(AccountRateLimitsUpdatedNotification);
opaque_struct!(AppListUpdatedNotification);
opaque_struct!(DeprecationNoticeNotification);
opaque_struct!(ConfigWarningNotification);
opaque_struct!(WindowsWorldWritableWarningNotification);
opaque_struct!(WindowsSandboxSetupCompletedNotification);
opaque_struct!(AccountLoginCompletedNotification);
opaque_struct!(AuthStatusChangeNotification);
opaque_struct!(LoginChatGptCompleteNotification);
opaque_struct!(SessionConfiguredNotification);
opaque_struct!(RawResponseItemCompletedNotification);
opaque_struct!(FuzzyFileSearchSessionUpdatedNotification);
opaque_struct!(FuzzyFileSearchSessionCompletedNotification);
