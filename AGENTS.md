# Repository Guidelines

## Project Structure & Module Organization
- Workspace root uses a virtual manifest (`Cargo.toml`) with members:
- `crates/sdk`: SDK library crate (`codex-app-server-sdk`).
- `crates/spark`: Spark CLI crate (`spark`) that depends on the SDK.
- `codex-app-server-sdk-macros`: proc-macro crate used by the SDK.
- `crates/sdk/src/lib.rs`: SDK crate exports.
- `crates/sdk/src/client/mod.rs`: async client, RPC lifecycle, handshake/readiness.
- `crates/sdk/src/api.rs`: high-level typed `Codex`/`Thread` convenience API.
- `crates/sdk/src/transport/`: `stdio` transport (default) and `ws` transport (feature-gated).
- `crates/sdk/src/protocol/`: typed request/response/notification/server-request models.
- `crates/sdk/src/events/mod.rs`: event parsing and enum mapping.
- `crates/sdk/src/error.rs`, `crates/sdk/src/compat.rs`: errors and CLI compatibility policy.
- `crates/sdk/examples/`: runnable SDK samples.
- `crates/sdk/tests/`: SDK protocol tests + ignored live integration tests.
- `crates/spark/src/main.rs`: Spark CLI entrypoint.
- `crates/spark/tests/`: Spark CLI integration tests.
- `adrs/`: architecture decision records.

## Build, Test, and Development Commands
- `cargo fmt --all`: format all Rust code.
- `cargo check --workspace`: compile validation for all workspace members.
- `cargo check -p codex-app-server-sdk --features ws`: SDK websocket feature validation.
- `cargo check -p spark --features ws`: Spark websocket mode validation.
- `cargo test --workspace -- --nocapture`: unit and non-ignored tests for all workspace members.
- `cargo test -p codex-app-server-sdk --test integration_stdio -- --ignored --nocapture`: real `codex app-server` SDK tests.
- `cargo test -p codex-app-server-sdk --test integration_api_stdio -- --ignored --nocapture`: real high-level API tests.
- `cargo test -p codex-app-server-sdk --features ws --test integration_ws -- --ignored --nocapture`: real websocket transport tests.
- `cargo test -p spark --test integration_spark -- --ignored --nocapture`: real `spark` CLI tests (resume/continue flows).
- `cargo run -p codex-app-server-sdk --example raw_fallback`: raw RPC smoke test.
- `cargo run -p codex-app-server-sdk --example turn_start_stream`: live turn streaming test.
- `cargo run -p spark -- "..."`: one-shot run (streamed by default) with fixed `gpt-5.3-codex-spark` + `xhigh`.
- `cargo run -p spark -- --stdio "..."`: one-shot run over stdio transport instead of default websocket.
- `cargo run -p spark -- --cwd <path> "..."`: one-shot run with explicit Codex working directory.
- `cargo run -p spark -- --final-response "..."`: one-shot run that only prints final response content.
- `cargo run -p spark -- --continue "..."`: continue the most recent recorded session.
- `cargo run -p spark -- --resume <session_id> "..."`: resume a specific session id.

## CI Merge Gate (Source of Truth)
- Required for merge:
- `cargo fmt --all`
- `cargo check --workspace`
- `cargo check -p codex-app-server-sdk --features ws`
- `cargo check -p spark --features ws`
- `cargo test --workspace -- --nocapture`
- Also required when touching protocol parsing, lifecycle, or transport:
- `cargo test -p codex-app-server-sdk --test integration_stdio -- --ignored --nocapture`
- If live tests fail, treat the failure as actionable and fix the underlying cause (do not dismiss as environmental).

## Agent Workflow
1. Preflight: `rustc --version`, `cargo --version`, `codex --version`.
2. Implement minimal typed changes first; keep compatibility fallbacks.
3. Validate in order: `fmt`, `check --workspace`, `check -p codex-app-server-sdk --features ws`, `check -p spark --features ws`, `test --workspace`, live integration tests.
4. If behavior changes, update `crates/sdk/examples/` and `README.md` in the same PR.

## Agent Communication & Verification
- Always run relevant tests/checks after code changes without waiting for user request; report results or why not run.
- Do not invent execution rules; if unsure, re-read `AGENTS.md`/`README.md`/CI docs before stating constraints.
- Avoid interim status narration during research; deliver one consolidated update with findings unless the user asks for step-by-step updates.

## Coding Style & Naming Conventions
- Follow `rustfmt` output (4-space indentation, trailing commas where applicable).
- Naming: `snake_case` for functions/modules/files, `UpperCamelCase` for types/enums, `UPPER_SNAKE_CASE` for constants.
- Keep protocol structs explicit and forward-compatible: preserve unknown fields via `extra` maps and `Unknown` variants.

## Testing Guidelines
- Use `#[test]` for pure protocol/unit behavior and `#[tokio::test]` for async flows.
- Name tests as behavior statements (for example, `model_list_typed_matches_raw`).
- Live tests must be `#[ignore]` unless they are deterministic in CI.
- For protocol changes, add serialization + event-path coverage.
- For lifecycle changes, test handshake invariant: `initialize()` then `initialized()` before normal RPC.

## Integration Test Prerequisites
- `codex` CLI must be installed and executable from `PATH`.
- `codex app-server` must start successfully on local machine.
- User must be authenticated (`chatgpt` or API key mode) for account/model/turn flows.
- Network access must be available for upstream model calls.
- Live SDK integration tests inherit host `HOME`/`CODEX_HOME` auth context by default; set `CODEX_SDK_TEST_ISOLATE_HOME=1` to opt into isolated home mode when debugging config-induced failures.
- If running `crates/sdk/examples/auth_api_key.rs`, set `OPENAI_API_KEY`.
- Known non-fatal runtime logs from app-server can appear; treat test assertions, not stderr noise, as the pass/fail signal.

## Commit & Pull Request Guidelines
- Git history currently has no commits; no repository-specific commit pattern exists yet.
- Use concise imperative subjects (recommended: Conventional Commits, e.g., `feat: add turn interrupt integration test`).
- PRs should include what changed, why, validation commands/results, protocol/API impact, and docs/example updates.

## Pull Request Definition of Done
- Code compiles and all required checks pass (see CI Merge Gate).
- New behavior has tests (unit and/or integration) and existing tests are updated.
- Public behavior changes are reflected in `README.md` and relevant `crates/sdk/examples/`.
- Compatibility implications are documented when changing protocol parsing or `crates/sdk/src/compat.rs`.
- No secrets are added to code, tests, examples, or logs.

## Release and Versioning Policy
- Version intent:
- `patch`: bug fix, no public API break.
- `minor`: additive public API or behavior-compatible expansion.
- `major` (or pre-1.0 designated breaking bump): removal/rename/semantic break.
- When changing tested CLI range in `crates/sdk/src/compat.rs`, update:
- `README.md` compatibility text.
- `AGENTS.md` runtime expectations.
- PR evidence with at least one ignored live integration test run.
- New typed protocol features must preserve raw fallback unless a replacement path is documented.

## Public API Change Rules
- Prefer additive changes over breaking changes.
- Do not remove or rename public items without a migration note.
- Preserve forward-compatibility behavior:
- unknown fields remain preserved via `extra`.
- unknown notifications/server requests map to `Unknown` variants.
- request envelopes continue omitting `jsonrpc`.
- Handshake invariants are API-level behavior and must remain stable unless intentionally versioned.

## Security & Configuration Tips
- Do not hardcode secrets; use environment variables (for example, `OPENAI_API_KEY`).
- Keep auth/token handling in runtime config only.
- Run live tests only in trusted environments because they execute a real local app-server process.

## Protocol Invariants (Do Not Break)
- JSON-RPC messages must omit the `jsonrpc` field.
- Maintain readiness gating: non-init calls are invalid before `initialized()`.
- Do not remove raw fallback APIs; they are required for protocol drift handling.
- Parse unknown notifications/server requests into explicit `Unknown` variants instead of failing hard.

## Known Runtime Expectations
- Live integration tests require local `codex app-server` and active auth.
- If local `~/.codex/config.toml` or `$CODEX_HOME/config.toml` contains unsupported keys, integration tests can fail with config-derivation errors; prefer isolating `HOME`/`CODEX_HOME` in test runtime env when validating SDK behavior independent of user config.
- `crates/sdk/examples/auth_api_key.rs` requires `OPENAI_API_KEY`.
- Compatibility policy is enforced in `crates/sdk/src/compat.rs`; update tests/docs when adjusting version ranges.
- With `ws` enabled, loopback websocket URLs auto-manage a persistent local daemon (`codex app-server --listen ...`) and write logs to `/tmp/codex-app-server-sdk/`.
- `CodexClient` provides high-level API entrypoints (`start_thread`, `resume_thread`, `as_api`) so stdio and ws clients can both use the same typed `run`/`run_streamed` thread flow.
- High-level API includes final-response shortcuts: `Thread::ask(...)`, `Codex::ask(...)`, and `Codex::ask_with_options(...)`, which return only the final agent message text.
- Use `ThreadOptions::builder()` for API-level thread defaults; it now covers protocol-oriented fields beyond CLI parity (for example `model_provider`, `personality`, `sandbox_policy`, collaboration mode payload, and config/dynamic tool extras).
- Use `TurnOptions::builder()` for per-turn output schema control; `output_schema` maps directly to app-server `turn/start.output_schema` and accepts either raw `serde_json::Value` or typed schemas via `output_schema_for::<T>()`.
- Typed schema generation is provided by `OpenAiSerializable` + `openai_json_schema_for::<T>()` (backed by `schemars`); derived schemas strip `$schema` metadata for OpenAI/Codex structured output compatibility.
- The `spark` binary supports fresh runs and session continuation (`--continue` for latest, `--resume <session_id>` for explicit ids), defaults to websocket transport at `ws://127.0.0.1:4222` (with `--stdio` override), sets Codex `cwd` to the invocation directory by default (override with `--cwd <path>`), streams `agentMessage` deltas to stdout by default, supports `--final-response` for final-message-only output, and always pins model + reasoning (`gpt-5.3-codex-spark`, `xhigh`).
- `spark --agent <name>` resolves `~/.codex/config.toml` under `[agents.<name>]`, reads `config_file` (relative to the declaring config file), and maps role config instructions into thread `developer_instructions` with precedence: `developer_instructions` -> `model_instructions_file` contents -> role `description`.
- On macOS, `spark` may resolve to an unrelated global Bun binary (`/usr/local/bin/spark`); verify with `which -a spark` and use `cargo run -p spark -- ...` or `./target/release/spark ...` to run the repository binary.

## Critical Paths and Review Focus
- High-risk paths:
- `crates/sdk/src/client/mod.rs`: state machine, request correlation, timeout/error semantics.
- `crates/sdk/src/protocol/*`: wire compatibility and serde mapping.
- `crates/sdk/src/events/mod.rs`: event decoding and unknown fallback paths.
- `crates/sdk/tests/integration_stdio.rs`: live behavior contract.
- For edits in these files, add explicit before/after behavior notes in the PR description.
