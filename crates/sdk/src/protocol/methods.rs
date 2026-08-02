//! Single source of truth for the client-to-server RPC method table.
//!
//! [`codex_rpc_table!`](crate::protocol::methods) is a callback-style macro
//! (like `server_request_table!` in [`crate::events`]): invoke it with the
//! name of a local macro that receives every row. Row kinds:
//!
//! - `typed fn_name, "wire/method", ParamsTy, ResultTy;` — a request that
//!   takes typed params.
//! - `null fn_name, "wire/method", ResultTy;` — a request sent with `null`
//!   params (no argument on the generated method).
//! - `alias fn_name => target, ParamsTy, ResultTy;` — a backward-compatible
//!   alias that delegates to another row's method and has no wire string of
//!   its own.
//!
//! Each row may be preceded by doc comments, which are attached to the
//! generated methods. The wire method strings live only here:
//! `crate::client` expands the table into `CodexClient`'s typed request
//! methods and `crate::api` expands it into the `ensure_initialized`
//! `Codex` forwards, so the two surfaces cannot drift apart. Adding an RPC
//! method is one new row.

macro_rules! codex_rpc_table {
    ($callback:ident) => {
        $callback! {
            typed thread_start, "thread/start",
                crate::protocol::requests::ThreadStartParams,
                crate::protocol::responses::ThreadResult;
            typed thread_resume, "thread/resume",
                crate::protocol::requests::ThreadResumeParams,
                crate::protocol::responses::ThreadResult;
            typed thread_fork, "thread/fork",
                crate::protocol::requests::ThreadForkParams,
                crate::protocol::responses::ThreadResult;
            typed thread_archive, "thread/archive",
                crate::protocol::requests::ThreadArchiveParams,
                crate::protocol::responses::ThreadArchiveResult;
            typed thread_name_set, "thread/name/set",
                crate::protocol::requests::ThreadSetNameParams,
                crate::protocol::responses::ThreadSetNameResult;
            typed thread_unarchive, "thread/unarchive",
                crate::protocol::requests::ThreadUnarchiveParams,
                crate::protocol::responses::ThreadUnarchiveResult;
            typed thread_compact_start, "thread/compact/start",
                crate::protocol::requests::ThreadCompactStartParams,
                crate::protocol::responses::ThreadCompactStartResult;
            typed thread_background_terminals_clean, "thread/backgroundTerminals/clean",
                crate::protocol::requests::ThreadBackgroundTerminalsCleanParams,
                crate::protocol::responses::ThreadBackgroundTerminalsCleanResult;
            typed thread_rollback, "thread/rollback",
                crate::protocol::requests::ThreadRollbackParams,
                crate::protocol::responses::ThreadRollbackResult;
            /// Lists recorded threads.
            typed thread_list, "thread/list",
                crate::protocol::requests::ThreadListParams,
                crate::protocol::responses::ThreadListResult;
            typed thread_loaded_list, "thread/loaded/list",
                crate::protocol::requests::ThreadLoadedListParams,
                crate::protocol::responses::ThreadLoadedListResult;
            typed thread_read, "thread/read",
                crate::protocol::requests::ThreadReadParams,
                crate::protocol::responses::ThreadReadResult;
            typed skills_list, "skills/list",
                crate::protocol::requests::SkillsListParams,
                crate::protocol::responses::SkillsListResult;
            typed skills_remote_list, "skills/remote/list",
                crate::protocol::requests::SkillsRemoteReadParams,
                crate::protocol::responses::SkillsRemoteReadResult;
            typed skills_remote_export, "skills/remote/export",
                crate::protocol::requests::SkillsRemoteWriteParams,
                crate::protocol::responses::SkillsRemoteWriteResult;
            typed app_list, "app/list",
                crate::protocol::requests::AppsListParams,
                crate::protocol::responses::AppsListResult;
            typed skills_config_write, "skills/config/write",
                crate::protocol::requests::SkillsConfigWriteParams,
                crate::protocol::responses::SkillsConfigWriteResult;
            typed turn_start, "turn/start",
                crate::protocol::requests::TurnStartParams,
                crate::protocol::responses::TurnResult;
            typed turn_steer, "turn/steer",
                crate::protocol::requests::TurnSteerParams,
                crate::protocol::responses::TurnSteerResult;
            typed turn_interrupt, "turn/interrupt",
                crate::protocol::requests::TurnInterruptParams,
                crate::protocol::shared::EmptyObject;
            typed review_start, "review/start",
                crate::protocol::requests::ReviewStartParams,
                crate::protocol::responses::ReviewStartResult;
            typed model_list, "model/list",
                crate::protocol::requests::ModelListParams,
                crate::protocol::responses::ModelListResult;
            typed experimental_feature_list, "experimentalFeature/list",
                crate::protocol::requests::ExperimentalFeatureListParams,
                crate::protocol::responses::ExperimentalFeatureListResult;
            typed collaboration_mode_list, "collaborationMode/list",
                crate::protocol::requests::CollaborationModeListParams,
                crate::protocol::responses::CollaborationModeListResult;
            typed mock_experimental_method, "mock/experimentalMethod",
                crate::protocol::requests::MockExperimentalMethodParams,
                crate::protocol::responses::MockExperimentalMethodResult;
            typed mcp_server_oauth_login, "mcpServer/oauth/login",
                crate::protocol::requests::McpServerOauthLoginParams,
                crate::protocol::responses::McpServerOauthLoginResult;
            typed mcp_server_status_list, "mcpServerStatus/list",
                crate::protocol::requests::ListMcpServerStatusParams,
                crate::protocol::responses::McpServerStatusListResult;
            typed windows_sandbox_setup_start, "windowsSandbox/setupStart",
                crate::protocol::requests::WindowsSandboxSetupStartParams,
                crate::protocol::responses::WindowsSandboxSetupStartResult;
            typed account_login_start, "account/login/start",
                crate::protocol::requests::LoginAccountParams,
                crate::protocol::responses::LoginAccountResult;
            typed account_login_cancel, "account/login/cancel",
                crate::protocol::requests::CancelLoginAccountParams,
                crate::protocol::shared::EmptyObject;
            typed feedback_upload, "feedback/upload",
                crate::protocol::requests::FeedbackUploadParams,
                crate::protocol::responses::FeedbackUploadResult;
            typed command_exec, "command/exec",
                crate::protocol::requests::CommandExecParams,
                crate::protocol::responses::CommandExecResult;
            typed config_read, "config/read",
                crate::protocol::requests::ConfigReadParams,
                crate::protocol::responses::ConfigReadResult;
            typed config_value_write, "config/value/write",
                crate::protocol::requests::ConfigValueWriteParams,
                crate::protocol::responses::ConfigValueWriteResult;
            typed config_batch_write, "config/batchWrite",
                crate::protocol::requests::ConfigBatchWriteParams,
                crate::protocol::responses::ConfigBatchWriteResult;
            typed account_read, "account/read",
                crate::protocol::requests::GetAccountParams,
                crate::protocol::responses::GetAccountResult;
            typed fuzzy_file_search_session_start, "fuzzyFileSearch/sessionStart",
                crate::protocol::requests::FuzzyFileSearchSessionStartParams,
                crate::protocol::responses::FuzzyFileSearchSessionStartResult;
            typed fuzzy_file_search_session_update, "fuzzyFileSearch/sessionUpdate",
                crate::protocol::requests::FuzzyFileSearchSessionUpdateParams,
                crate::protocol::responses::FuzzyFileSearchSessionUpdateResult;
            typed fuzzy_file_search_session_stop, "fuzzyFileSearch/sessionStop",
                crate::protocol::requests::FuzzyFileSearchSessionStopParams,
                crate::protocol::responses::FuzzyFileSearchSessionStopResult;
            null config_mcp_server_reload, "config/mcpServer/reload",
                crate::protocol::shared::EmptyObject;
            null account_logout, "account/logout",
                crate::protocol::shared::EmptyObject;
            null account_rate_limits_read, "account/rateLimits/read",
                crate::protocol::responses::AccountRateLimitsReadResult;
            null config_requirements_read, "configRequirements/read",
                crate::protocol::responses::ConfigRequirementsReadResult;
            /// Backward-compatible alias for the previous name of
            /// `skills_remote_list`.
            alias skills_remote_read => skills_remote_list,
                crate::protocol::requests::SkillsRemoteReadParams,
                crate::protocol::responses::SkillsRemoteReadResult;
            /// Backward-compatible alias for the previous name of
            /// `skills_remote_export`.
            alias skills_remote_write => skills_remote_export,
                crate::protocol::requests::SkillsRemoteWriteParams,
                crate::protocol::responses::SkillsRemoteWriteResult;
        }
    };
}

pub(crate) use codex_rpc_table;
