# ADR 0002: Always-On WebSocket Transport and Default Live Integration Tests

- Status: Accepted
- Date: 2026-02-24

## Context

The SDK currently supports both `stdio` and websocket transports, but websocket was feature-gated.
Live integration tests existed, but were marked `#[ignore]` and required explicit opt-in.

This split created two problems:

- Production runtime behavior could differ by build flags.
- The default test command did not exercise live app-server flows.

## Decision

Adopt an always-on transport and test policy:

- Remove websocket feature-gating from `codex-app-server-sdk` and `spark`.
- Build websocket transport support in all default builds.
- Run SDK and Spark live integration tests by default (remove `#[ignore]`).
- Update repository agent guidance to treat full integration test execution as standard validation on all code changes.

## Consequences

Positive:

- One consistent runtime surface across builds.
- Regressions in app-server integration are caught in normal test runs.
- Agent and CI workflows align with real deployment behavior.

Tradeoffs:

- Local and CI test runs now depend on `codex app-server`, auth, and network being available.
- Failing integration environments must be treated as actionable setup/runtime issues, not silently skipped.

## Notes

- `stdio` and websocket transports remain distinct code paths, but both are now always compiled and tested.
- Spark continues to default to websocket transport and keeps `--stdio` as an explicit override.
