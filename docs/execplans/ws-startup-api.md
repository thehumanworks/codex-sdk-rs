# Separate websocket startup from websocket connection

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

There is no repository-local planning contract such as `.agents/PLANS.md` in this repository as of 2026-03-14, so this document follows `/Users/mish/.agents/skills/exec-plan/references/PLANS.md`.

## Purpose / Big Picture

After this change, SDK users can do three distinct things with the websocket transport instead of relying on one overloaded convenience path: connect to an already running websocket app-server, start a websocket app-server explicitly, and choose whether that started server should run as a detached daemon or as an SDK-owned child process. This matters because `0.0.0.0` is a bind address, not a client destination, and the current `start_and_connect_ws` interface overloads one URL for both roles. Success is visible when a user can start a server with separate `listen_url` and `connect_url`, including `listen_url = ws://0.0.0.0:PORT` with `connect_url = ws://127.0.0.1:PORT`, and the SDK test suite proves that existing compatible servers are reused while conflicting listeners fail clearly.

## Progress

- [x] (2026-03-14 20:04Z) Inspect the current websocket client and daemon management code in `crates/sdk/src/client/mod.rs`, `crates/sdk/src/transport/ws_daemon.rs`, `crates/sdk/src/api.rs`, `crates/sdk/tests/integration_ws.rs`, and `crates/spark/src/main.rs`.
- [x] (2026-03-14 20:04Z) Confirm that there is no repo-local planning contract file that overrides the global ExecPlan instructions.
- [x] (2026-03-14 20:17Z) Define the additive websocket startup API with `WsStartConfig`, `WsStartMode`, and `WsServerHandle`, while keeping `connect_ws` connect-only and preserving `start_and_connect_ws` as the loopback convenience wrapper.
- [x] (2026-03-14 20:17Z) Implement the startup API in the transport and client layers, including explicit daemon and blocking modes, `reuse_existing`, separate `listen_url` and `connect_url`, and owned-child shutdown via process-group termination for blocking mode.
- [x] (2026-03-14 20:17Z) Update high-level API wrappers, public exports, examples, README text, the ADR, and runtime guidance so users see the new entry points and semantics.
- [x] (2026-03-14 20:17Z) Add and pass websocket tests that prove loopback reuse, connect-only behavior, explicit exposed-bind startup, blocking lifecycle ownership, and conflict handling.
- [x] (2026-03-14 20:17Z) Run formatting and validation commands through the full workspace test suite and record the outcomes here.

## Surprises & Discoveries

- Observation: The current SDK already has the reuse-vs-connect-only split in behavior, but the public API hides it behind `start_and_connect_ws` and a single `WsConfig { url, env, options }`.
  Evidence: `CodexClient::connect_ws` only calls `connect_ws_transport`, while `CodexClient::start_and_connect_ws` calls `ensure_local_ws_app_server` first in `crates/sdk/src/client/mod.rs`.

- Observation: The current daemon manager intentionally refuses to manage `0.0.0.0` and any non-loopback or `wss://` URL.
  Evidence: `parse_managed_ws_target` returns `Ok(None)` unless the scheme is `ws` and the host is loopback or `localhost` in `crates/sdk/src/transport/ws_daemon.rs`.

- Observation: Killing the direct child process was not sufficient to make blocking startup behave like SDK-owned lifecycle management in practice.
  Evidence: The first version of `start_ws_blocking_returns_owned_handle_and_can_shutdown` kept the websocket port occupied after `WsServerHandle::shutdown()`. The fix was to start the child in its own process group and terminate that process group during shutdown.

## Decision Log

- Decision: Keep `connect_ws` as the connect-only API and add explicit startup APIs instead of broadening `start_and_connect_ws` to auto-manage `0.0.0.0`.
  Rationale: `0.0.0.0` is a listen address, not a connect target. Splitting startup from connection removes the semantic overload and keeps the safe default intact.
  Date/Author: 2026-03-14 / Codex

- Decision: Treat daemon startup and SDK-owned startup as explicit modes of the new startup path.
  Rationale: The user asked for a daemon default with an alternative for callers that want the SDK to own lifecycle. Exposing both modes is clearer than a hidden boolean policy.
  Date/Author: 2026-03-14 / Codex

- Decision: Keep `start_ws` as the daemon-default convenience entry point, with `start_ws_daemon` and `start_ws_blocking` as the explicit forms.
  - Superseded (0.6.0): `start_ws` was removed as a zero-value alias of `start_ws_daemon`; the explicit forms are the only entry points. See docs/complexity-assessment.md issue 1.3.
  Rationale: This preserves the ergonomic default the user asked for while keeping the high-stakes lifecycle distinction visible in the API surface.
  Date/Author: 2026-03-14 / Codex

- Decision: Terminate the entire process group for blocking startup shutdown instead of only killing the immediate child PID.
  Rationale: Live testing showed that killing only the child did not reliably free the websocket port. Process-group termination matches the intended “SDK owns this server lifecycle” behavior better.
  Date/Author: 2026-03-14 / Codex

## Outcomes & Retrospective

The SDK now exposes explicit websocket startup primitives in addition to the legacy loopback convenience wrapper. Callers can use `connect_ws` for pure connection, `start_ws_daemon` for detached startup, and `start_ws_blocking` for SDK-owned lifecycle. The new startup config separates `listen_url` from `connect_url`, which makes exposed binds such as `0.0.0.0` explicit instead of overloaded.

The most important implementation detail discovered during execution was that blocking-mode lifecycle ownership required process-group shutdown, not just killing the first child PID. Live tests caught that gap before completion. The result now matches the purpose of the change: startup and connection are separated, loopback convenience still exists, and the new behavior is demonstrated by passing websocket integration tests, Spark integration tests, and the full workspace suite.

## Context and Orientation

The websocket client surface currently lives in `crates/sdk/src/client/mod.rs`. That file defines `WsConfig`, which today mixes three concerns into one struct: where the client connects, environment variables used if a server must be spawned, and timeout options for the client. The transport code is split by direction. `crates/sdk/src/transport/ws.rs` opens a websocket connection and translates websocket frames to the SDK transport channels. `crates/sdk/src/transport/ws_daemon.rs` is the current process-management helper; it probes a target URL, starts `codex app-server --listen ...` for loopback `ws://` URLs, and waits until the target becomes reachable.

The high-level convenience API in `crates/sdk/src/api.rs` mirrors the lower-level `CodexClient` constructors. Public exports are re-exported from `crates/sdk/src/lib.rs`. Live websocket behavior is covered in `crates/sdk/tests/integration_ws.rs`. The CLI crate `crates/spark/src/main.rs` uses the SDK constructors directly, so any interface change that affects websocket startup must keep the Spark contract valid. The repository README at `README.md`, the crate README at `crates/sdk/README.md`, and the example `crates/sdk/examples/ws_persistent.rs` document the public behavior and were updated in the same change.

In this plan, a "daemon" means a detached `codex app-server` process that the SDK starts and then leaves running after the current client disconnects. A "blocking" startup mode means the SDK starts a child process and keeps ownership of that child through a returned handle so the caller can shut it down or let it die with the process. A "compatible existing server" means a websocket listener at the target `connect_url` that completes a websocket handshake; a different kind of listener on the same port is a conflict and must fail clearly instead of being reused silently.

## Plan of Work

First, add a dedicated startup configuration type in `crates/sdk/src/client/mod.rs` that separates the address the server should listen on from the address the client should use to connect. The startup config must also carry environment variables and a `reuse_existing` policy. Add an explicit startup mode enum with daemon and blocking variants. Keep `WsConfig` as the connect-only client configuration so existing code that only connects to a server remains straightforward.

Next, refactor `crates/sdk/src/transport/ws_daemon.rs` into a more general websocket server startup helper that can probe a `connect_url`, decide whether an existing listener is reusable, and either spawn a detached daemon or return an SDK-owned child handle. The startup helper must reject invalid websocket URLs up front, allow non-loopback `listen_url` values when the caller provided a separate `connect_url`, and continue to fail fast when the port is occupied by a non-websocket service. For daemon mode, the helper should preserve the current log-file behavior under `/tmp/codex-app-server-sdk/`. For blocking mode, return a handle that exposes the effective `listen_url`, `connect_url`, ownership status, and a shutdown method for owned children.

Then update `CodexClient` and `Codex` in `crates/sdk/src/client/mod.rs` and `crates/sdk/src/api.rs` to expose the new startup entry points. Keep `start_and_connect_ws` as a small wrapper over the managed loopback path so current callers keep working, but make the new public direction explicit in docs and examples.

Finally, update `README.md` and `crates/sdk/examples/ws_persistent.rs` to show the split between starting and connecting, then extend `crates/sdk/tests/integration_ws.rs` with behavior-oriented tests. Those tests must cover reuse of an existing compatible websocket server, failure of connect-only mode when no server is running, explicit startup with separate listen/connect URLs, and conflict handling when an occupied port is not a websocket app-server.

## Concrete Steps

From the repository root `/Users/mish/dev/codex-sdk-rs`, inspect the current websocket code:

    rg -n "WsConfig|connect_ws|start_and_connect_ws|ensure_local_ws_app_server" crates/sdk/src crates/sdk/tests README.md crates/spark/src/main.rs

Create the new startup types and transport helpers, then format and run the relevant websocket-facing validation:

    cargo fmt --all
    cargo check --workspace
    cargo test -p codex-app-server-sdk --test integration_ws -- --nocapture
    cargo test -p spark --test integration_spark -- --nocapture

If the broader workspace remains green within reasonable time, run the full suite as the final proof:

    cargo test --workspace -- --nocapture

Expected outcomes:

    The SDK compiles with new websocket startup types and methods.
    The websocket integration tests show that a compatible existing server is reused, connect-only mode does not start a daemon, and conflicting listeners fail clearly.
    The README examples refer to explicit startup APIs instead of implying that one websocket URL can safely represent both listen and connect behavior.

## Validation and Acceptance

Acceptance is behavioral, not just structural. A user should be able to start a websocket app-server explicitly and then connect to it using separate URLs when needed. One acceptance path is a loopback daemon: starting with default startup config should either reuse an existing local websocket app-server or start `codex app-server --listen ws://127.0.0.1:4222`, then `connect_ws(WsConfig::default())` should succeed. Another acceptance path is an exposed bind: starting with `listen_url = ws://0.0.0.0:PORT` and `connect_url = ws://127.0.0.1:PORT` should succeed, proving the SDK no longer assumes one URL serves both purposes. A conflict path must also be covered: if the target port is occupied by a non-websocket listener, startup must return a clear error instead of silently reusing or hanging.

Tests must demonstrate these behaviors. At minimum, `cargo test -p codex-app-server-sdk --test integration_ws -- --nocapture` must pass with newly added websocket startup coverage. Because Spark exposes websocket startup semantics to end users, `cargo test -p spark --test integration_spark -- --nocapture` should also pass after any CLI updates.

## Idempotence and Recovery

This work is additive and should be safe to repeat. Re-running formatting and tests is safe. If a spawned websocket daemon remains running after a test failure, rerunning the startup path should reuse that compatible daemon instead of creating duplicates. If a blocking-start test leaves a child process alive, shut it down through the returned handle before rerunning the test. If a port conflict test fails because a prior listener was not cleaned up, release the temporary listener and rerun only that test first before rerunning the full websocket suite.

## Artifacts and Notes

Record the final validation transcripts here once implementation is complete. Keep only the short lines that prove success, such as the test target names and their pass counts.

Validation completed on 2026-03-14:

    cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.80s

    cargo test -p codex-app-server-sdk --test integration_ws -- --nocapture
    test result: ok. 7 passed; 0 failed

    cargo test -p spark --test integration_spark -- --nocapture
    test result: ok. 3 passed; 0 failed

    cargo test --workspace -- --nocapture
    test result: ok. 25 passed in sdk unit tests
    test result: ok. 8 passed in integration_api_stdio
    test result: ok. 4 passed in integration_stdio
    test result: ok. 7 passed in integration_ws
    test result: ok. 8 passed in protocol_roundtrip
    test result: ok. 2 passed in schema_reexport
    test result: ok. 53 passed in spark unit tests
    test result: ok. 3 passed in integration_spark

## Interfaces and Dependencies

In `crates/sdk/src/client/mod.rs`, define new public websocket startup types along these lines:

    pub struct WsStartConfig {
        pub listen_url: String,
        pub connect_url: String,
        pub env: HashMap<String, String>,
        pub reuse_existing: bool,
    }

    pub enum WsStartMode {
        Daemon,
        Blocking,
    }

    pub struct WsServerHandle { ... }

`WsServerHandle` must report whether the SDK started a new child or attached to an existing server, and it must allow callers to retrieve the effective `connect_url`. `CodexClient` should expose `start_ws`, `start_ws_daemon`, and `start_ws_blocking` entry points, with `start_and_connect_ws` delegating through them for compatibility. The transport startup helper in `crates/sdk/src/transport/ws_daemon.rs` should continue to use `codex app-server --listen <listen_url>` and write daemon logs under `/tmp/codex-app-server-sdk/`.

Revision note: Created this ExecPlan before implementation to make the interface, transport, test, and documentation work restartable for a stateless contributor.
Revision note: Updated after implementation to record the final API, the process-group shutdown fix for blocking mode, the documentation changes, and the passing validation evidence.
