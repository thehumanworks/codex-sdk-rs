# ADR 0003: Feature-Complete API Surface and Per-Turn Override Model

- Status: Accepted
- Date: 2026-02-24

## Context

The SDK had strong low-level coverage in `CodexClient`, but the higher-level `Codex`/`Thread`
abstractions only exposed a subset of app-server flows. Per-turn configuration was limited to
`output_schema`, and server-request auto-handling was only implemented for ChatGPT token refresh.

This created three practical gaps:

- Full app-server flows were not uniformly accessible through high-level abstractions.
- Per-turn option control was incomplete for users who needed turn-specific overrides.
- Approval/tool server requests required manual plumbing except for one request type.

## Decision

Adopt a feature-complete SDK surface for known app-server RPCs:

- `Codex` forwards the full typed RPC method surface (mirroring `CodexClient`) after
  initialization/readiness checks.
- `Thread` adds lifecycle and turn-control helpers:
  `set_name`, `read`, `archive`, `unarchive`, `rollback`, `compact_start`, `steer`, `interrupt`.
- `TurnOptions` expands from schema-only to full per-turn overrides for:
  `cwd`, `model`, `model_provider`, reasoning effort/summary, personality, approval policy,
  sandbox policy, collaboration mode, network/web-search flags, additional directories,
  and arbitrary raw extra fields.
- Server-request auto-handling supports typed handlers for all known approval/tool request
  families (apply patch, exec command, command execution approval, file change approval,
  tool user input, dynamic tool call) in addition to auth token refresh.

## Consequences

Positive:

- High-level and low-level SDK paths are aligned; users can stay in typed abstractions longer.
- Turn-scoped behavior can be controlled explicitly without mutating thread defaults.
- Interactive server-request flows can be implemented with typed callbacks instead of ad-hoc event code.

Tradeoffs:

- API surface area is larger and requires broader validation.
- Some protocol payloads remain intentionally forward-compatible (`extra`/`Unknown`) until upstream
  schemas are fully stabilized and documented.

## Notes

- Forward compatibility remains a hard requirement: raw fallback and unknown preservation are retained.
- Added tests cover per-turn override mapping and server-request auto-handler behavior.
