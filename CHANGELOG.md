# Changelog

## codex-app-server-sdk 0.6.0 — UNRELEASED

> **This release contains breaking changes.** They are intentional
> complexity reductions from the audit in `docs/complexity-assessment.md`;
> no deprecation shims were kept. Migration notes are inline below.

### Breaking changes

- **Removed `CodexClient::start_ws` and `Codex::start_ws`.** Both were pure
  aliases. Migrate: call `start_ws_daemon` (identical behavior).
- **Removed `CodexClient::{start_thread, resume_thread, resume_thread_by_id,
  resume_latest_thread}`.** These duplicated the `Codex` API. Migrate:
  `client.as_api().start_thread(...)`, or construct a `Codex` directly.
- **Removed `TurnOptions::{with_output_schema, with_output_schema_for,
  with_model, with_working_directory}`.** They were a third configuration
  style beside public fields and the builder. Migrate:
  `TurnOptions::builder().output_schema(...).build()` etc.
- **Removed `pub type RunResult`.** Migrate: use `api::Turn` directly.
- **Removed `protocol::shared::{JsonRpcNotification, JsonRpcResponse}`.**
  Never constructed by the SDK; the wire is built from typed requests.
- **Removed `TurnStartParams::collaboration_mode` (`Option<String>`).** The
  field was always `None` and its type contradicted the structured object
  actually sent. Collaboration mode travels via `TurnOptions` /
  `ThreadOptions` (see bug fix below).
- **Removed `schema::{serialize_openai_value, deserialize_openai_value}`.**
  They were aliases of `serde_json::{to_value, from_value}`. The trait
  conveniences `to_openai_value` / `from_openai_value` remain.
- **Removed the `serde_yaml` dependency** (declared but unused; the crate is
  deprecated upstream).

### Bug fixes

- **`thread/start` now sends `collaborationMode`.** Previously a
  `collaboration_mode` set in `ThreadOptions` was silently dropped on
  `thread/start` (while `thread/resume` and `turn/start` sent it). Payload
  snapshot tests now pin the exact key sets of all three requests.

### Improvements

- **Wire enums are single-sourced.** `ApprovalMode`, `SandboxMode`,
  `ModelReasoningEffort`, `ModelReasoningSummary`, `Personality`,
  `WebSearchMode`, and `CollaborationModeKind` are generated from one table
  each (`wire_enum!`), and now expose public `as_str()`, `Display`,
  `FromStr` (accepting the wire spellings), and a `VARIANTS` const.
  New `ModelVerbosity` enum (previously duplicated in each CLI) with the
  same surface. `api::UnknownItem` is now exported from the crate root.
- **One RPC method table.** The full 43-method surface (plus the two
  `skills_remote_*` aliases) is defined once in `protocol::methods` and
  expanded into both `CodexClient` and `Codex`; a compile-time test proves
  the two types expose the same set, so the surfaces can no longer drift.
- **Table-driven server-request handling and notification parsing.** The
  seven approval/tool server requests and the 40 typed notifications are
  each declared in one table that generates the enums, parsers, handler
  storage, `set_*`/`clear_*`/`respond_*` methods, and dispatch.
  `ServerNotification::method_name()` is new.
- **Shared transport plumbing.** stdio and websocket transports share one
  reader/writer implementation with named channel capacities; websocket
  text and binary frames go through a single path.
- **Options builders are generated** from one field table; adding an option
  is now a one-row change. Thread/turn request encoding shares one
  extras inserter over an explicit, unit-tested merge of turn-over-thread
  precedence.

## luna 0.3.0 — UNRELEASED

- Hand-rolled enum parsers and the local `ModelVerbosity` deleted in favor
  of the SDK's `FromStr`/`VARIANTS` (error messages unchanged in shape,
  now generated).
- **Fix:** auth env vars (`OPENAI_API_KEY`, `CODEX_API_KEY`,
  `CODEX_ID_TOKEN`, `CODEX_ACCESS_TOKEN`) are now forwarded to any
  `codex app-server` daemon the SDK spawns; previously a freshly spawned
  daemon started unauthenticated.

## agx 0.2.0 — UNRELEASED

- New `--cwd <path>` flag (defaults to the invocation directory); the
  thread working directory was previously never set.
- Removed the vestigial `nickname_candidates` agent-config field (parsed
  and validated but never used; unknown keys in agent files remain
  tolerated).
- Loopback detection is no longer string-matched in agx; the SDK's
  managed-target validation decides daemon startability, and connect
  errors are preserved in the fallback failure message.
- Same daemon auth-env forwarding fix as luna.
