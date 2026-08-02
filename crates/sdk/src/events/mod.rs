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

/// Declarative table of server notifications: one row per
/// `(variant, wire method, payload type)`. Emits the `ServerNotification`
/// enum, `parse_notification`, the `method_name` accessor, and the list of
/// method names used by the completeness test, so a row cannot half-land.
macro_rules! notification_table {
    ( $( $variant:ident => ($method:literal, $payload:ty) ),+ $(,)? ) => {
        #[derive(Debug, Clone)]
        pub enum ServerNotification {
            $( $variant($payload), )+
            Unknown { method: String, params: Value },
        }

        pub fn parse_notification(
            method: String,
            params: Value,
        ) -> Result<ServerNotification, ClientError> {
            let event = match method.as_str() {
                $( $method => ServerNotification::$variant(decode(params)?), )+
                _ => ServerNotification::Unknown { method, params },
            };
            Ok(event)
        }

        impl ServerNotification {
            /// The wire method string for this notification, or `None` for
            /// `Unknown`.
            pub fn method_name(&self) -> Option<&'static str> {
                match self {
                    $( Self::$variant(_) => Some($method), )+
                    Self::Unknown { .. } => None,
                }
            }

            #[cfg(test)]
            pub(crate) const METHOD_NAMES: &'static [&'static str] = &[ $( $method ),+ ];
        }
    };
}

notification_table! {
    Error => ("error", n::ErrorNotification),
    ThreadStarted => ("thread/started", n::ThreadStartedNotification),
    ThreadArchived => ("thread/archived", n::ThreadLifecycleNotification),
    ThreadUnarchived => ("thread/unarchived", n::ThreadLifecycleNotification),
    ThreadClosed => ("thread/closed", n::ThreadLifecycleNotification),
    ThreadNameUpdated => ("thread/name/updated", n::ThreadNameUpdatedNotification),
    ThreadStatusChanged => ("thread/status/changed", n::ThreadStatusChangedNotification),
    ThreadTokenUsageUpdated => ("thread/tokenUsage/updated", n::ThreadTokenUsageUpdatedNotification),
    TurnStarted => ("turn/started", n::TurnStartedNotification),
    TurnCompleted => ("turn/completed", n::TurnCompletedNotification),
    TurnDiffUpdated => ("turn/diff/updated", n::TurnDiffUpdatedNotification),
    TurnPlanUpdated => ("turn/plan/updated", n::TurnPlanUpdatedNotification),
    ItemStarted => ("item/started", n::ItemLifecycleNotification),
    ItemCompleted => ("item/completed", n::ItemLifecycleNotification),
    RawResponseItemCompleted => ("rawResponseItem/completed", n::RawResponseItemCompletedNotification),
    ItemAgentMessageDelta => ("item/agentMessage/delta", n::DeltaNotification),
    ItemPlanDelta => ("item/plan/delta", n::DeltaNotification),
    ItemCommandExecutionOutputDelta => ("item/commandExecution/outputDelta", n::DeltaNotification),
    ItemCommandExecutionTerminalInteraction => ("item/commandExecution/terminalInteraction", n::DeltaNotification),
    ItemFileChangeOutputDelta => ("item/fileChange/outputDelta", n::DeltaNotification),
    ItemMcpToolCallProgress => ("item/mcpToolCall/progress", n::DeltaNotification),
    ItemReasoningSummaryTextDelta => ("item/reasoning/summaryTextDelta", n::DeltaNotification),
    ItemReasoningSummaryPartAdded => ("item/reasoning/summaryPartAdded", n::DeltaNotification),
    ItemReasoningTextDelta => ("item/reasoning/textDelta", n::DeltaNotification),
    McpServerOauthLoginCompleted => ("mcpServer/oauthLogin/completed", n::McpServerOauthLoginCompletedNotification),
    AccountUpdated => ("account/updated", n::AccountUpdatedNotification),
    AccountRateLimitsUpdated => ("account/rateLimits/updated", n::AccountRateLimitsUpdatedNotification),
    AppListUpdated => ("app/list/updated", n::AppListUpdatedNotification),
    ContextCompacted => ("thread/compacted", n::DeltaNotification),
    DeprecationNotice => ("deprecationNotice", n::DeprecationNoticeNotification),
    ConfigWarning => ("configWarning", n::ConfigWarningNotification),
    WindowsWorldWritableWarning => ("windows/worldWritableWarning", n::WindowsWorldWritableWarningNotification),
    WindowsSandboxSetupCompleted => ("windowsSandbox/setupCompleted", n::WindowsSandboxSetupCompletedNotification),
    AccountLoginCompleted => ("account/login/completed", n::AccountLoginCompletedNotification),
    AuthStatusChange => ("authStatusChange", n::AuthStatusChangeNotification),
    LoginChatGptComplete => ("loginChatGptComplete", n::LoginChatGptCompleteNotification),
    SessionConfigured => ("sessionConfigured", n::SessionConfiguredNotification),
    FuzzyFileSearchSessionUpdated => ("fuzzyFileSearch/sessionUpdated", n::FuzzyFileSearchSessionUpdatedNotification),
    FuzzyFileSearchSessionCompleted => ("fuzzyFileSearch/sessionCompleted", n::FuzzyFileSearchSessionCompletedNotification),
    ServerRequestResolved => ("serverRequest/resolved", n::ServerRequestResolvedNotification),
}

/// The single source of truth for the seven server-initiated requests.
///
/// Each row carries every name the request needs across the SDK:
/// `Variant { method, params type, response type, handler field, set/clear
/// registration methods, respond wrapper, error-context string }`. The table
/// is expanded here (enum + parser) and in `client::server_requests` (handler
/// storage, registration API, auto-dispatch), so a row cannot half-land.
macro_rules! server_request_table {
    ($callback:ident) => {
        $callback! {
            ChatgptAuthTokensRefresh {
                method: "account/chatgptAuthTokens/refresh",
                params: ChatgptAuthTokensRefreshParams,
                response: ChatgptAuthTokensRefreshResponse,
                handler: chatgpt_auth_tokens_refresh,
                set: set_chatgpt_auth_tokens_refresh_handler,
                clear: clear_chatgpt_auth_tokens_refresh_handler,
                respond: respond_chatgpt_auth_tokens_refresh,
                context: "chatgptAuthTokens refresh",
            },
            ApplyPatchApproval {
                method: "applyPatchApproval",
                params: ApplyPatchApprovalParams,
                response: ApplyPatchApprovalResponse,
                handler: apply_patch_approval,
                set: set_apply_patch_approval_handler,
                clear: clear_apply_patch_approval_handler,
                respond: respond_apply_patch_approval,
                context: "applyPatchApproval",
            },
            ExecCommandApproval {
                method: "execCommandApproval",
                params: ExecCommandApprovalParams,
                response: ExecCommandApprovalResponse,
                handler: exec_command_approval,
                set: set_exec_command_approval_handler,
                clear: clear_exec_command_approval_handler,
                respond: respond_exec_command_approval,
                context: "execCommandApproval",
            },
            CommandExecutionRequestApproval {
                method: "item/commandExecution/requestApproval",
                params: CommandExecutionRequestApprovalParams,
                response: CommandExecutionRequestApprovalResponse,
                handler: command_execution_request_approval,
                set: set_command_execution_request_approval_handler,
                clear: clear_command_execution_request_approval_handler,
                respond: respond_command_execution_request_approval,
                context: "item/commandExecution/requestApproval",
            },
            FileChangeRequestApproval {
                method: "item/fileChange/requestApproval",
                params: FileChangeRequestApprovalParams,
                response: FileChangeRequestApprovalResponse,
                handler: file_change_request_approval,
                set: set_file_change_request_approval_handler,
                clear: clear_file_change_request_approval_handler,
                respond: respond_file_change_request_approval,
                context: "item/fileChange/requestApproval",
            },
            ToolRequestUserInput {
                method: "item/tool/requestUserInput",
                params: ToolRequestUserInputParams,
                response: ToolRequestUserInputResponse,
                handler: tool_request_user_input,
                set: set_tool_request_user_input_handler,
                clear: clear_tool_request_user_input_handler,
                respond: respond_tool_request_user_input,
                context: "item/tool/requestUserInput",
            },
            DynamicToolCall {
                method: "item/tool/call",
                params: DynamicToolCallParams,
                response: DynamicToolCallResponse,
                handler: dynamic_tool_call,
                set: set_dynamic_tool_call_handler,
                clear: clear_dynamic_tool_call_handler,
                respond: respond_dynamic_tool_call,
                context: "item/tool/call",
            },
        }
    };
}

pub(crate) use server_request_table;

/// Expands the server-request table into the `ServerRequestEvent` enum and
/// `parse_server_request`.
macro_rules! define_server_request_events {
    ( $( $variant:ident {
        method: $method:literal,
        params: $params:ident,
        response: $response:ident,
        handler: $handler:ident,
        set: $set:ident,
        clear: $clear:ident,
        respond: $respond:ident,
        context: $context:literal,
    } ),+ $(,)? ) => {
        #[derive(Debug, Clone)]
        pub enum ServerRequestEvent {
            $( $variant {
                id: RequestId,
                params: sr::$params,
            }, )+
            Unknown {
                id: RequestId,
                method: String,
                params: Value,
            },
        }

        pub fn parse_server_request(
            id: RequestId,
            method: String,
            params: Value,
        ) -> Result<ServerRequestEvent, ClientError> {
            let req = match method.as_str() {
                $( $method => ServerRequestEvent::$variant {
                    id,
                    params: decode(params)?,
                }, )+
                _ => ServerRequestEvent::Unknown { id, method, params },
            };
            Ok(req)
        }
    };
}

server_request_table!(define_server_request_events);

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ClientError> {
    serde_json::from_value(params).map_err(ClientError::Serialization)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_notification_row_parses_and_round_trips_its_method_name() {
        // A superset params object satisfying the required fields of every
        // notification payload type; unrecognized keys land in each payload's
        // flattened `extra` map.
        let params = json!({
            "error": { "message": "m" },
            "thread": { "id": "thr_1" },
            "threadId": "thr_1",
            "name": "n",
            "turn": { "id": "turn_1" },
            "turnId": "turn_1",
            "requestId": 1
        });

        for method in ServerNotification::METHOD_NAMES {
            let parsed = parse_notification(method.to_string(), params.clone())
                .unwrap_or_else(|err| panic!("failed to parse `{method}`: {err}"));
            assert_eq!(
                parsed.method_name(),
                Some(*method),
                "method name did not round-trip for `{method}`"
            );
        }
    }

    #[test]
    fn unknown_notification_has_no_method_name() {
        let parsed = parse_notification("no/such/method".to_string(), json!({})).expect("parse");
        assert!(matches!(parsed, ServerNotification::Unknown { .. }));
        assert_eq!(parsed.method_name(), None);
    }
}
