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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatgptAuthTokensRefreshParams {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_account_id: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatgptAuthTokensRefreshResponse {
    pub id_token: String,
    pub access_token: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

opaque_struct!(ApplyPatchApprovalParams);
opaque_struct!(ApplyPatchApprovalResponse);
opaque_struct!(ExecCommandApprovalParams);
opaque_struct!(ExecCommandApprovalResponse);
opaque_struct!(CommandExecutionRequestApprovalParams);
opaque_struct!(CommandExecutionRequestApprovalResponse);
opaque_struct!(FileChangeRequestApprovalParams);
opaque_struct!(FileChangeRequestApprovalResponse);
opaque_struct!(ToolRequestUserInputParams);
opaque_struct!(ToolRequestUserInputResponse);
opaque_struct!(DynamicToolCallParams);
opaque_struct!(DynamicToolCallResponse);
