# Repository Guidelines

## Project Structure & Module Organization
- `src/lib.rs`: crate exports.
- `src/client/mod.rs`: async client, RPC lifecycle, handshake/readiness.
- `src/api.rs`: high-level typed `Codex`/`Thread` convenience API.
- `src/transport/`: `stdio` transport (default) and `ws` transport (feature-gated).
- `src/protocol/`: typed request/response/notification/server-request models.
- `src/events/mod.rs`: event parsing and enum mapping.
- `src/error.rs`, `src/compat.rs`: errors and CLI compatibility policy.
- `examples/`: runnable samples.
- `tests/`: protocol tests + ignored live integration tests.

## Build, Test, and Development Commands
- `cargo fmt --all`: format all Rust code.
- `cargo check`: compile validation.
- `cargo check --features ws`: websocket feature validation.
- `cargo test -- --nocapture`: unit and non-ignored tests.
- `cargo test --test integration_stdio -- --ignored --nocapture`: real `codex app-server` tests.
- `cargo test --test integration_api_stdio -- --ignored --nocapture`: real high-level API tests.
- `cargo test --features ws --test integration_ws -- --ignored --nocapture`: real websocket transport tests.
- `cargo run --example raw_fallback`: raw RPC smoke test.
- `cargo run --example turn_start_stream`: live turn streaming test.

## CI Merge Gate (Source of Truth)
- Required for merge:
- `cargo fmt --all`
- `cargo check`
- `cargo check --features ws`
- `cargo test -- --nocapture`
- Also required when touching protocol parsing, lifecycle, or transport:
- `cargo test --test integration_stdio -- --ignored --nocapture`
- If live tests fail, label failure as either:
- `environmental` (auth/session/network/runtime problem) or `regression` (SDK behavior change).

## Agent Workflow
1. Preflight: `rustc --version`, `cargo --version`, `codex --version`.
2. Implement minimal typed changes first; keep compatibility fallbacks.
3. Validate in order: `fmt`, `check`, `check --features ws`, `test`, live integration tests.
4. If behavior changes, update `examples/` and `README.md` in the same PR.

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
- If running `examples/auth_api_key.rs`, set `OPENAI_API_KEY`.
- Known non-fatal runtime logs from app-server can appear; treat test assertions, not stderr noise, as the pass/fail signal.

## Commit & Pull Request Guidelines
- Git history currently has no commits; no repository-specific commit pattern exists yet.
- Use concise imperative subjects (recommended: Conventional Commits, e.g., `feat: add turn interrupt integration test`).
- PRs should include what changed, why, validation commands/results, protocol/API impact, and docs/example updates.

## Pull Request Definition of Done
- Code compiles and all required checks pass (see CI Merge Gate).
- New behavior has tests (unit and/or integration) and existing tests are updated.
- Public behavior changes are reflected in `README.md` and relevant `examples/`.
- Compatibility implications are documented when changing protocol parsing or `src/compat.rs`.
- No secrets are added to code, tests, examples, or logs.

## Release and Versioning Policy
- Version intent:
- `patch`: bug fix, no public API break.
- `minor`: additive public API or behavior-compatible expansion.
- `major` (or pre-1.0 designated breaking bump): removal/rename/semantic break.
- When changing tested CLI range in `src/compat.rs`, update:
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
- `auth_api_key` example requires `OPENAI_API_KEY`.
- Compatibility policy is enforced in `src/compat.rs`; update tests/docs when adjusting version ranges.
- With `ws` enabled, loopback websocket URLs auto-manage a persistent local daemon (`codex app-server --listen ...`) and write logs to `/tmp/codex-app-server-sdk/`.

## Critical Paths and Review Focus
- High-risk paths:
- `src/client/mod.rs`: state machine, request correlation, timeout/error semantics.
- `src/protocol/*`: wire compatibility and serde mapping.
- `src/events/mod.rs`: event decoding and unknown fallback paths.
- `tests/integration_stdio.rs`: live behavior contract.
- For edits in these files, add explicit before/after behavior notes in the PR description.
