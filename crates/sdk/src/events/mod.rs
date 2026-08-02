use serde_json::Value;

use crate::error::ClientError;
use crate::protocol::notifications as n;
use crate::protocol::server_requests as sr;
use crate::protocol::shared::RequestId;

pub mod render;

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Notification(ServerNotification),
    ServerRequest(ServerRequestEvent),
    TransportClosed,
}

#[derive(Debug, Clone)]
pub enum ServerNotification {
    Error(n::ErrorNotification),
    ThreadStarted(n::ThreadStartedNotification),
    ThreadArchived(n::ThreadLifecycleNotification),
    ThreadUnarchived(n::ThreadLifecycleNotification),
    ThreadClosed(n::ThreadLifecycleNotification),
    ThreadNameUpdated(n::ThreadNameUpdatedNotification),
    ThreadStatusChanged(n::ThreadStatusChangedNotification),
    ThreadTokenUsageUpdated(n::ThreadTokenUsageUpdatedNotification),
    TurnStarted(n::TurnStartedNotification),
    TurnCompleted(n::TurnCompletedNotification),
    TurnDiffUpdated(n::TurnDiffUpdatedNotification),
    TurnPlanUpdated(n::TurnPlanUpdatedNotification),
    ItemStarted(n::ItemLifecycleNotification),
    ItemCompleted(n::ItemLifecycleNotification),
    RawResponseItemCompleted(n::RawResponseItemCompletedNotification),
    ItemAgentMessageDelta(n::DeltaNotification),
    ItemPlanDelta(n::DeltaNotification),
    ItemCommandExecutionOutputDelta(n::DeltaNotification),
    ItemCommandExecutionTerminalInteraction(n::DeltaNotification),
    ItemFileChangeOutputDelta(n::DeltaNotification),
    ItemMcpToolCallProgress(n::DeltaNotification),
    ItemReasoningSummaryTextDelta(n::DeltaNotification),
    ItemReasoningSummaryPartAdded(n::DeltaNotification),
    ItemReasoningTextDelta(n::DeltaNotification),
    McpServerOauthLoginCompleted(n::McpServerOauthLoginCompletedNotification),
    AccountUpdated(n::AccountUpdatedNotification),
    AccountRateLimitsUpdated(n::AccountRateLimitsUpdatedNotification),
    AppListUpdated(n::AppListUpdatedNotification),
    ContextCompacted(n::DeltaNotification),
    DeprecationNotice(n::DeprecationNoticeNotification),
    ConfigWarning(n::ConfigWarningNotification),
    WindowsWorldWritableWarning(n::WindowsWorldWritableWarningNotification),
    WindowsSandboxSetupCompleted(n::WindowsSandboxSetupCompletedNotification),
    AccountLoginCompleted(n::AccountLoginCompletedNotification),
    AuthStatusChange(n::AuthStatusChangeNotification),
    LoginChatGptComplete(n::LoginChatGptCompleteNotification),
    SessionConfigured(n::SessionConfiguredNotification),
    FuzzyFileSearchSessionUpdated(n::FuzzyFileSearchSessionUpdatedNotification),
    FuzzyFileSearchSessionCompleted(n::FuzzyFileSearchSessionCompletedNotification),
    ServerRequestResolved(n::ServerRequestResolvedNotification),
    Unknown { method: String, params: Value },
}

#[derive(Debug, Clone)]
pub enum ServerRequestEvent {
    ChatgptAuthTokensRefresh {
        id: RequestId,
        params: sr::ChatgptAuthTokensRefreshParams,
    },
    ApplyPatchApproval {
        id: RequestId,
        params: sr::ApplyPatchApprovalParams,
    },
    ExecCommandApproval {
        id: RequestId,
        params: sr::ExecCommandApprovalParams,
    },
    CommandExecutionRequestApproval {
        id: RequestId,
        params: sr::CommandExecutionRequestApprovalParams,
    },
    FileChangeRequestApproval {
        id: RequestId,
        params: sr::FileChangeRequestApprovalParams,
    },
    ToolRequestUserInput {
        id: RequestId,
        params: sr::ToolRequestUserInputParams,
    },
    DynamicToolCall {
        id: RequestId,
        params: sr::DynamicToolCallParams,
    },
    Unknown {
        id: RequestId,
        method: String,
        params: Value,
    },
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ClientError> {
    serde_json::from_value(params).map_err(ClientError::Serialization)
}

pub fn parse_notification(
    method: String,
    params: Value,
) -> Result<ServerNotification, ClientError> {
    let event = match method.as_str() {
        "error" => ServerNotification::Error(decode(params)?),
        "thread/started" => ServerNotification::ThreadStarted(decode(params)?),
        "thread/archived" => ServerNotification::ThreadArchived(decode(params)?),
        "thread/unarchived" => ServerNotification::ThreadUnarchived(decode(params)?),
        "thread/closed" => ServerNotification::ThreadClosed(decode(params)?),
        "thread/name/updated" => ServerNotification::ThreadNameUpdated(decode(params)?),
        "thread/status/changed" => ServerNotification::ThreadStatusChanged(decode(params)?),
        "thread/tokenUsage/updated" => ServerNotification::ThreadTokenUsageUpdated(decode(params)?),
        "turn/started" => ServerNotification::TurnStarted(decode(params)?),
        "turn/completed" => ServerNotification::TurnCompleted(decode(params)?),
        "turn/diff/updated" => ServerNotification::TurnDiffUpdated(decode(params)?),
        "turn/plan/updated" => ServerNotification::TurnPlanUpdated(decode(params)?),
        "item/started" => ServerNotification::ItemStarted(decode(params)?),
        "item/completed" => ServerNotification::ItemCompleted(decode(params)?),
        "rawResponseItem/completed" => {
            ServerNotification::RawResponseItemCompleted(decode(params)?)
        }
        "item/agentMessage/delta" => ServerNotification::ItemAgentMessageDelta(decode(params)?),
        "item/plan/delta" => ServerNotification::ItemPlanDelta(decode(params)?),
        "item/commandExecution/outputDelta" => {
            ServerNotification::ItemCommandExecutionOutputDelta(decode(params)?)
        }
        "item/commandExecution/terminalInteraction" => {
            ServerNotification::ItemCommandExecutionTerminalInteraction(decode(params)?)
        }
        "item/fileChange/outputDelta" => {
            ServerNotification::ItemFileChangeOutputDelta(decode(params)?)
        }
        "item/mcpToolCall/progress" => ServerNotification::ItemMcpToolCallProgress(decode(params)?),
        "item/reasoning/summaryTextDelta" => {
            ServerNotification::ItemReasoningSummaryTextDelta(decode(params)?)
        }
        "item/reasoning/summaryPartAdded" => {
            ServerNotification::ItemReasoningSummaryPartAdded(decode(params)?)
        }
        "item/reasoning/textDelta" => ServerNotification::ItemReasoningTextDelta(decode(params)?),
        "mcpServer/oauthLogin/completed" => {
            ServerNotification::McpServerOauthLoginCompleted(decode(params)?)
        }
        "account/updated" => ServerNotification::AccountUpdated(decode(params)?),
        "account/rateLimits/updated" => {
            ServerNotification::AccountRateLimitsUpdated(decode(params)?)
        }
        "app/list/updated" => ServerNotification::AppListUpdated(decode(params)?),
        "thread/compacted" => ServerNotification::ContextCompacted(decode(params)?),
        "deprecationNotice" => ServerNotification::DeprecationNotice(decode(params)?),
        "configWarning" => ServerNotification::ConfigWarning(decode(params)?),
        "windows/worldWritableWarning" => {
            ServerNotification::WindowsWorldWritableWarning(decode(params)?)
        }
        "windowsSandbox/setupCompleted" => {
            ServerNotification::WindowsSandboxSetupCompleted(decode(params)?)
        }
        "account/login/completed" => ServerNotification::AccountLoginCompleted(decode(params)?),
        "authStatusChange" => ServerNotification::AuthStatusChange(decode(params)?),
        "loginChatGptComplete" => ServerNotification::LoginChatGptComplete(decode(params)?),
        "sessionConfigured" => ServerNotification::SessionConfigured(decode(params)?),
        "fuzzyFileSearch/sessionUpdated" => {
            ServerNotification::FuzzyFileSearchSessionUpdated(decode(params)?)
        }
        "fuzzyFileSearch/sessionCompleted" => {
            ServerNotification::FuzzyFileSearchSessionCompleted(decode(params)?)
        }
        "serverRequest/resolved" => ServerNotification::ServerRequestResolved(decode(params)?),
        _ => ServerNotification::Unknown { method, params },
    };
    Ok(event)
}

pub fn parse_server_request(
    id: RequestId,
    method: String,
    params: Value,
) -> Result<ServerRequestEvent, ClientError> {
    let req = match method.as_str() {
        "account/chatgptAuthTokens/refresh" => ServerRequestEvent::ChatgptAuthTokensRefresh {
            id,
            params: decode(params)?,
        },
        "applyPatchApproval" => ServerRequestEvent::ApplyPatchApproval {
            id,
            params: decode(params)?,
        },
        "execCommandApproval" => ServerRequestEvent::ExecCommandApproval {
            id,
            params: decode(params)?,
        },
        "item/commandExecution/requestApproval" => {
            ServerRequestEvent::CommandExecutionRequestApproval {
                id,
                params: decode(params)?,
            }
        }
        "item/fileChange/requestApproval" => ServerRequestEvent::FileChangeRequestApproval {
            id,
            params: decode(params)?,
        },
        "item/tool/requestUserInput" => ServerRequestEvent::ToolRequestUserInput {
            id,
            params: decode(params)?,
        },
        "item/tool/call" => ServerRequestEvent::DynamicToolCall {
            id,
            params: decode(params)?,
        },
        _ => ServerRequestEvent::Unknown { id, method, params },
    };
    Ok(req)
}
