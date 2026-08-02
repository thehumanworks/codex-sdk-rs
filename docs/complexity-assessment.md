# Complexity Assessment & Simplification Backlog

> ## Implementation status addendum (2026-08-02)
>
> This backlog has been substantially **implemented** on this branch (SDK
> 0.6.0, breaking changes taken directly — no deprecation shims; see
> `CHANGELOG.md`). Status by issue:
>
> - **Done:** 0.1, 0.2, 1.1, 1.2, 1.3, 1.5, 2.1, 2.2, 2.3, 2.4, 2.5, 2.6,
>   2.7, 2.8, 3.8, 4.1, 4.2, 4.3, and the error/env/schema portions of 4.6.
> - **Superseded by the spark → luna rewrite** (which landed on main during
>   implementation): 0.3–0.6, 1.4, 3.2, 3.5, 3.6, 3.7 — the spark bugs and
>   spark-specific refactors. Luna's rewrite independently fixed B3/B4/B5;
>   B2 (daemon env) and B6 (JSON status casing) were still present in luna
>   and are fixed on this branch. Luna also uses clap and a modular layout,
>   covering the intent of 3.2/3.5. The new `crates/luna/src/cli.rs`
>   (1,624 lines) and `doctor.rs` (718 lines) have NOT been audited — a
>   fresh assessment pass over luna is recommended.
> - **Partially superseded:** 3.1/3.3 — the SDK gained `events/render.rs`
>   (shared renderer) upstream; agx still renders by hand and could adopt
>   it. A shared `cli-core` crate remains optional now that only two CLIs
>   (luna, agx) exist with less overlap.
> - **Remaining open:** 3.4 (derive `Serialize` on `ThreadItem`), 4.4
>   (serde-derived item parsing), 4.5 (promote extra-only options to typed
>   protocol fields), 4.7 (agent-config unification, luna-vs-agx flavor),
>   and the opaque-notification collapse deferred from 4.6.
>
> Line references in the original text below are against commit `f3a50a6`
> and have drifted; the issue intent still governs.

_Assessed at commit `f3a50a6` (workspace v0.5.1). Line references are against that commit._

## 1. Executive summary — complexity vs. feature scope and user-visible value

The workspace delivers three user-visible things:

1. **`codex-app-server-sdk`** (published on crates.io) — a typed Tokio client for the Codex
   app-server JSON-RPC protocol over stdio/websocket, plus a high-level `Codex`/`Thread` API.
2. **`spark`** — a one-shot/resume CLI over the SDK.
3. **`agx`** — an agent-profile-focused CLI over the SDK.

Total: ~13,750 lines of Rust, of which ~11,300 are production code. For the feature scope —
one protocol client and two thin CLIs — this is roughly **2× the code the value requires**.
The excess is not in any single over-engineered abstraction; it is in **systematic
repetition and missing sharing**:

- **The 43-method RPC table is hand-maintained twice** (`client/mod.rs:812-1077` and
  `api.rs:1085-1291`, ~473 lines) with two different macro dialects. It has already drifted
  (`thread_list` hand-expanded in `api.rs:1077-1083`; `skills_remote_*` aliases duplicated
  verbatim in both files).
- **Each of the 7 server-request handlers is written out five times** (type aliases, `Inner`
  fields, set/clear pairs, respond wrappers, dispatch arms) — ~359 lines in
  `client/mod.rs` where ~115 would do. Adding a handler means touching 8 places with no
  compile error if one is missed.
- **The high-level options layer shadows the protocol layer by hand.** `ThreadOptions` /
  `TurnOptions` and their builders are ~530 lines of mechanical setters; ten options exist
  only to be stuffed into the untyped `extra` map as string keys, via three copy-pasted
  `build_*_params` functions. This copy-paste has already produced a real bug (§2, B1).
- **The two CLIs share zero code.** Event rendering, enum parsing, transport setup, path
  resolution, preview truncation, and agent-config loading are each implemented twice —
  sometimes with *divergent* behavior (two different streaming dedup state machines, two
  incompatible `--agent` config systems, two different default models). spark additionally
  hand-rolls a 640-line argument parser for 33 flags while agx already uses clap.
- **Seven string enums are triple-maintained**: serde renames in the SDK, a private
  `as_str()` in the SDK, and hand-written parsers re-implemented in spark (~90 lines) and
  again in agx — solely because `as_str` is private and `FromStr` is missing.

**Testing posture:** the quality gate leans almost entirely on live integration tests that
require a real `codex` binary and network (`AGENTS.md`); there are only ~28 SDK unit tests.
~60% of the RPC surface (26 of 43 methods) has zero test coverage, spark's stdout/`--json`
output has no tests at all, and agx has no integration test crate. Several issues below
therefore *require adding tests before refactoring* — that requirement is stated per issue.

**Projected outcome if the backlog below lands:** ≈ **−2,300 net LOC** (non-breaking waves
alone ≈ −1,800), `client/mod.rs` ~1,460 → ~700, `api.rs` ~3,440 → ~2,400, `spark/main.rs`
~3,300 → ~1,900, `agx/main.rs` ~1,560 → ~1,000, plus the elimination of four classes of
silent-drift bugs (method tables, handler tables, enum spellings, CLI behavior forks).

---

## 2. Verified bugs found during the assessment (fix first)

These were found while auditing for complexity and are confirmed against the source. They
are filed as Wave 0 issues below; they are listed here so nobody mistakes them for
refactoring niceties.

- **B1 — `collaboration_mode` silently dropped on `thread/start`.**
  `build_thread_start_params` (`api.rs:1920-1990`) never encodes
  `ThreadOptions::collaboration_mode`, while `build_thread_resume_params` (`api.rs:2041`)
  and `build_turn_start_params` (`api.rs:2128`) both do. A collaboration mode set on a
  fresh thread only takes effect from the first turn's override path.
- **B2 — daemon spawn env not forwarded.** Both CLIs pass `env: Default::default()` into
  `WsConfig` (spark `main.rs:795,809`; agx builds no env at all), so `OPENAI_API_KEY` etc.
  never reach a spawned `codex app-server` daemon. Root cause is an SDK API trap:
  `WsConfig.env` is only honored by `start_and_connect_ws`, silently ignored by
  `connect_ws` (`client/mod.rs:477-499`).
- **B3 — `spark sessions` ignores `$CODEX_WEB_SERVER_URL`.** The sessions path
  (`spark/main.rs:292`) skips `resolve_websocket_url` that `exec`/`start` use
  (`spark/main.rs:310-315`).
- **B4 — `ensure_authenticated` silently succeeds when unauthenticated.**
  `spark/main.rs:829-860` returns `Ok(())` when the server is not logged in and no env
  credentials exist; the failure surfaces later as an opaque turn error.
- **B5 — sugar flags silently override explicit `--config` keys.** In
  `build_thread_config` (`spark/main.rs:2114-2145`) `--web-search-mode`,
  `--config-profile`, `--model-verbosity`, `--sandbox-network-*`,
  `--sandbox-writable-root` are inserted after (and therefore beat) user-provided
  `--config KEY=VALUE` / `--config-json` entries, with no error or warning.
- **B6 — spark `--json` leaks Rust `Debug` formatting.** `thread_item_to_json`
  (`spark/main.rs:686,703,714`) serializes three status enums with `format!("{:?}", …)`,
  producing PascalCase Rust identifiers in an otherwise camelCase JSON contract.

---

## 3. How to use this backlog

- Issues are grouped into **waves**. Issues within a wave are independent and can be
  executed **in parallel by separate agents in fresh sessions** unless a per-issue
  *Conflicts* note says otherwise. Later waves depend on earlier ones only where the
  *Depends on* field says so.
- Every issue must satisfy the **global quality gate** (from `AGENTS.md`):
  `cargo fmt --all`, `cargo check --workspace`, `cargo test --workspace -- --nocapture`.
  Issues touching protocol parsing, lifecycle, or transport must also pass the four live
  integration suites (`integration_stdio`, `integration_api_stdio`, `integration_ws`,
  `integration_spark`). Per-issue requirements below are *additional*.
- `codex-app-server-sdk` is a **published crate**. "Unused in this repo" ≠ removable.
  Issues marked *semver: breaking* must land behind a 0.6 version bump; until then use
  `#[deprecated]`. A `cargo public-api` (or `cargo doc` diff) check is required wherever
  noted.
- Estimated LOC deltas are net across the workspace and approximate.

---

## Wave 0 — Bug fixes (all parallel, all small)

### Issue 0.1 — Fix: `collaboration_mode` dropped by `thread/start`
- **Files:** `crates/sdk/src/api.rs:1920-1990`
- **Task:** Add the `extra["collaborationMode"] = collaboration_mode.as_value()` insertion
  to `build_thread_start_params`, mirroring `build_thread_resume_params` (`api.rs:2041-2046`).
- **Tests required:** Extend the existing `thread/start` payload snapshot tests
  (`api.rs:3142-3307`) with a case asserting a set `collaboration_mode` appears in the
  `thread/start` params. Add the symmetric assertion for resume (already covered) so the
  three builders are pinned consistently.
- **Acceptance:** New test fails before the fix, passes after. No other payload keys change.
- **Risk:** Low. **LOC:** +6. **Conflicts:** touches the same region as Issue 2.3 — land first.

### Issue 0.2 — Fix: forward env to spawned WS daemons; document `WsConfig.env` semantics
- **Files:** `crates/spark/src/main.rs:792-823`, `crates/agx/src/main.rs:729-753`,
  `crates/sdk/src/client/mod.rs:131-160`
- **Task:** (a) In both CLIs, populate `WsConfig.env` (at minimum pass through
  `OPENAI_API_KEY` / `CODEX_API_KEY` when present, matching what
  `integration_spark.rs:21-26` does for stdio). (b) In the SDK, doc-comment `WsConfig.env`:
  "applied only when the SDK starts a daemon (`start_and_connect_ws` / `start_ws_*`);
  ignored by `connect_ws`". Full relocation of the field is deferred to Issue 4.6.
- **Tests required:** Unit test in each CLI asserting the constructed `WsConfig` carries
  the expected env when the variables are set (inject via parameterized helper, do not
  mutate real process env in tests without serialization).
- **Acceptance:** A daemon spawned by `spark`/`agx` inherits the auth env vars.
  `integration_spark::spark_start_command_readies_websocket_daemon` still passes.
- **Risk:** Low-Medium (behavioral). **LOC:** +25.

### Issue 0.3 — Fix: route `spark sessions` through `resolve_websocket_url`
- **Files:** `crates/spark/src/main.rs:291-315`
- **Task:** Make the `sessions` path use `resolve_websocket_url` (flag > env > default)
  exactly like `exec`/`start`. While there, stop computing a WS URL on the stdio path
  (dead value, `main.rs:313-315`) by making it an `Option`.
- **Tests required:** Unit test asserting sessions honors `--ws-url` and
  `$CODEX_WEB_SERVER_URL` precedence (the precedence function already has tests at
  `main.rs:2736-2755`; add a dispatch-level test).
- **Acceptance:** All three subcommands share one URL-resolution call site.
- **Risk:** Low. **LOC:** −5.

### Issue 0.4 — Fix: fail loudly when unauthenticated in spark
- **Files:** `crates/spark/src/main.rs:829-860`, `:207-241`
- **Task:** When `account/read` reports not-logged-in and neither env credential pair is
  present, return an error ("not authenticated; run `codex login` or set OPENAI_API_KEY")
  instead of `Ok(())`. Add a `Runtime` variant to `SparkError` and reclassify the five
  stream-failure sites currently mislabeled `SparkError::Config`
  (`main.rs:545,551,613,619,625`).
- **Tests required:** Unit test for the error message/variant mapping. Verify
  `integration_spark` still passes (it runs authenticated).
- **Acceptance:** Unauthenticated runs fail fast with an actionable message; turn failures
  no longer render under a config-error label.
- **Risk:** Low. **LOC:** +10.

### Issue 0.5 — Fix: make `--config` vs. sugar-flag precedence explicit
- **Files:** `crates/spark/src/main.rs:2077-2152`
- **Task:** Decide and enforce one rule: explicit `--config KEY=VALUE` /`--config-json`
  wins over sugar flags (`--web-search-mode`, `--config-profile`, `--model-verbosity`,
  `--sandbox-network-*`, `--sandbox-writable-root`), OR conflicting keys are an error.
  Recommended: error on conflict (deterministic, no silent surprises). Replace the five
  bespoke `if let` blocks with a `[(flag_value, config_key)]` table loop.
- **Tests required:** `build_thread_config` currently has **no tests** — add precedence
  tests first (pin current behavior), then change behavior in the same PR with updated
  expectations, documenting the change in README.
- **Acceptance:** `spark exec --config web_search=live --web-search-mode disabled …` has a
  documented, tested outcome (error or explicit precedence), not a silent override.
- **Risk:** Medium (behavioral). **LOC:** −25.

### Issue 0.6 — Fix: spark `--json` status casing (fold into Issue 3.4 if scheduled together)
- **Files:** `crates/spark/src/main.rs:634-790`
- **Task:** Stop serializing statuses with `format!("{:?}")`; use the SDK kebab/camel
  spellings. If Issue 3.4 (serde-derived `ThreadItem` serialization) is being executed in
  the same cycle, skip this and let 3.4 subsume it.
- **Tests required:** Golden test of the `--json` line for a command-execution item with a
  status, asserting the wire casing.
- **Acceptance:** No Rust identifier casing in `--json` output; README notes the contract.
- **Risk:** Low (output contract change — call it out in the PR body). **LOC:** ~0.

---

## Wave 1 — Dead code & zero-risk hygiene (all parallel)

### Issue 1.1 — Remove unused `serde_yaml` dependency
- **Files:** `crates/sdk/Cargo.toml:25`, `Cargo.lock`
- **Task:** Delete the dependency. It has zero usages in the workspace and the crate is
  officially unmaintained (RUSTSEC-2024-0320 advisory class).
- **Acceptance:** `grep -r serde_yaml crates codex-app-server-sdk-macros --include='*.rs'`
  returns nothing; workspace builds and tests pass.
- **Risk:** None. **LOC:** −1 (and one fewer supply-chain dependency).

### Issue 1.2 — Hoist the four copies of `opaque_struct!` into `protocol/mod.rs`
- **Files:** `crates/sdk/src/protocol/{mod,requests,responses,notifications,server_requests}.rs`
- **Task:** Define the macro once in `protocol/mod.rs` (`pub(crate) use opaque_struct;`),
  delete the four byte-identical copies (`requests.rs:4-13`, `responses.rs:4-13`,
  `notifications.rs:7-16`, `server_requests.rs:4-13`).
- **Tests required:** None new; `protocol_roundtrip.rs` must stay green.
- **Acceptance:** Exactly one `macro_rules! opaque_struct` in the workspace; no public API
  change (`cargo public-api` diff empty).
- **Risk:** None. **LOC:** −30.

### Issue 1.3 — Deprecate/remove dead public SDK surface; export `UnknownItem`
- **Files:** `crates/sdk/src/api.rs`, `crates/sdk/src/lib.rs`, `crates/sdk/src/client/mod.rs`,
  `crates/sdk/src/protocol/shared.rs`
- **Task:**
  - Delete `pub type RunResult = Turn;` (`api.rs:698`, re-export `lib.rs:20`) — zero uses.
  - Delete `TurnOptions::{with_output_schema, with_output_schema_for, with_model,
    with_working_directory}` (`api.rs:486-505`) — zero external uses; they are a third,
    inconsistent configuration style beside public fields and the builder.
  - `#[deprecated(note = "use start_ws_daemon")]` on `CodexClient::start_ws`
    (`client/mod.rs:482-484`) and `Codex::start_ws` (`api.rs:1008-1010`) — pure aliases,
    zero callers. (Note `docs/execplans/ws-startup-api.md:42` documented keeping it — update
    that doc in this PR.)
  - Delete `Codex::skills_remote_read`/`skills_remote_write` duplicates in `api.rs:1268-1280`
    only if Issue 2.5's shared table doesn't subsume them; otherwise leave for 2.5.
  - Delete the four `CodexClient::{start_thread,resume_thread,resume_thread_by_id,
    resume_latest_thread}` forwarders (`client/mod.rs:529-543`) — a fourth copy of the same
    entry points; callers use `.as_api()`.
  - `#[deprecated]` `JsonRpcNotification`/`JsonRpcResponse` (`protocol/shared.rs:24-34`) —
    zero references; the wire is built from `json!` literals.
  - Add `UnknownItem` to the `lib.rs:19-29` re-export list (currently `ThreadItem::Unknown`
    is unnameable downstream without the `api::` path).
  - Delete the dead vestigial field `TurnStartParams::collaboration_mode: Option<String>`
    (`protocol/requests.rs:280`) — always `None`, wrong type vs. the object actually sent
    in `extra`.
- **Tests required:** `cargo public-api` (or `cargo doc` output diff) attached to the PR
  showing exactly the intended removals/deprecations and nothing else.
- **Acceptance:** Workspace + examples compile; README contains no references to removed
  items; deprecations carry actionable `note`s.
- **Risk:** Low (semver-minor deprecations + removals of never-published-in-docs items —
  reviewer must confirm the removals are acceptable pre-1.0). **LOC:** −60.

### Issue 1.4 — spark dead code sweep
- **Files:** `crates/spark/src/main.rs`
- **Task:** Remove the undocumented `--sessions` flag alias and the redundant
  `sessions: bool` field (`:1239,1266-1270,1805-1809` — `command_kind` is authoritative);
  simplify the unreachable output-schema precedence (`:2158-2163`, both-set is already
  rejected at `:1793-1797`); remove the `#[allow(clippy::useless_conversion)]` in tests by
  adding an `fn args(&[&str]) -> Vec<String>` helper.
- **Tests required:** Update the two `--sessions`-alias unit tests to expect rejection (or
  delete them); all other parser tests unchanged.
- **Acceptance:** `spark sessions` (subcommand) unchanged; `spark --sessions` now errors
  with the standard unknown-option message.
- **Risk:** Low (removes an undocumented alias — note in PR body). **LOC:** −45.

### Issue 1.5 — agx: wire up or delete `nickname_candidates`
- **Files:** `crates/agx/src/main.rs:114,199-246,1063-1149`
- **Task:** The field is parsed and validated but never used by any production code path.
  Either plumb it into `LoadedAgent`/`build_thread_config` with a defined meaning
  (preferred only if the Codex app-server actually consumes it — check the protocol), or
  delete the field, its validator, and its tests.
- **Tests required:** Whichever branch: tests must reflect the final behavior; no
  parse-and-discard remains.
- **Acceptance:** `grep nickname_candidates` shows either a full production path or nothing.
- **Risk:** Low. **LOC:** −30 (delete branch).

---

## Wave 2 — Mechanical deduplication in the SDK

> These remove the four "silent drift" repetition classes. 2.1–2.4 are parallel-safe
> (different regions); 2.5 must follow 2.4; 2.3 must follow 0.1.

### Issue 2.1 — One source of truth for the seven wire enums; public `as_str`/`FromStr`
- **Files:** `crates/sdk/src/api.rs:23-156`, `crates/spark/src/main.rs:182-197,1930-2018`,
  `crates/agx/src/main.rs:65-81,353-359`
- **Task:** Introduce a local `macro_rules! wire_enum!` emitting, from one variant table
  per enum: the enum, serde renames, **public** `as_str()`, `Display`, `FromStr` (accepting
  the serde spellings, including `xhigh`), and a `pub const VARIANTS: &[&str]` for CLI help
  text. Apply to `ApprovalMode`, `SandboxMode`, `ModelReasoningEffort`,
  `ModelReasoningSummary`, `Personality`, `WebSearchMode`, `CollaborationModeKind`. Move
  `ModelVerbosity` (currently duplicated in both CLIs) into the SDK with the same
  treatment. Then delete spark's six `parse_*` functions + `web_search_mode_as_str` and
  agx's `web_search_mode_as_str` + `ModelVerbosity`, replacing them with `FromStr` and a
  friendly error built from `VARIANTS`.
- **Tests required:** Round-trip test per enum: for every variant,
  `Enum::from_str(v.as_str()) == v` and serde JSON round-trip equals `as_str`. Keep
  spark's existing parser unit tests (`main.rs:2803-2887`) passing against the new
  `FromStr` (adapt call sites, preserve the "expected one of: …" message content).
- **Acceptance:** Zero enum-spelling code outside the SDK; adding a variant requires
  editing exactly one table.
- **Risk:** Low. **LOC:** ≈ −175 net. **Conflicts:** none (top of `api.rs`).

### Issue 2.2 — Generate the options builders from one macro table
- **Files:** `crates/sdk/src/api.rs:286-629`
- **Task:** Replace the 266 lines of hand-written `ThreadOptionsBuilder`/`TurnOptionsBuilder`
  setters (15 of 20 turn setters are byte-identical to thread counterparts) with a single
  `macro_rules! options_builder!` that generates struct fields + builder from a
  `(field, type, setter_kind)` table. Public method names and signatures must be preserved
  exactly (the builders are published API).
- **Tests required:** Existing builder tests (`api.rs:3142-3391`) unchanged and green;
  `cargo public-api` diff empty.
- **Acceptance:** Adding an option = one table row (plus its `build_*_params` handling
  until Issue 4.5 lands).
- **Risk:** Low. **LOC:** ≈ −215. **Conflicts:** same file as 2.1/2.3 — coordinate merge
  order, regions are disjoint.

### Issue 2.3 — Extract the shared `extra`-key inserter from the three `build_*_params`
- **Files:** `crates/sdk/src/api.rs:1920-2180`
- **Depends on:** Issue 0.1 (bug fix lands first so this refactor is behavior-preserving).
- **Task:** Extract `insert_common_thread_extras(extra: &mut Map<…>, opts: &ResolvedOptions)`
  covering the nine keys currently copy-pasted across `build_thread_start_params` /
  `build_thread_resume_params` / `build_turn_start_params` (`skipGitRepoCheck`,
  `webSearchMode`, `webSearchEnabled`, `networkAccessEnabled`, `additionalDirectories`,
  `dynamicTools`, `experimentalRawEvents`, `collaborationMode`, `sandboxPolicy` where
  applicable). Split `build_turn_start_params` into (a) `TurnOptions`-over-`ThreadOptions`
  merge producing `ResolvedOptions`, (b) wire encoding — so precedence is testable in
  isolation.
- **Tests required:** **Before refactoring**, add payload snapshot tests for
  `thread/resume` and `turn/start` mirroring the existing `thread/start` snapshots
  (`api.rs:3142-3307`), pinning the exact key sets (note: start does *not* send
  `sandboxPolicy` in extra; resume does). Then refactor with snapshots unchanged.
- **Acceptance:** Each wire key is produced in exactly one place; per-method key-set
  differences are explicit (visible parameters, not copy-paste divergence).
- **Risk:** Low-Medium. **LOC:** ≈ −90.

### Issue 2.4 — Table-ize the server-request handler machinery
- **Files:** `crates/sdk/src/client/mod.rs:29-97,438-445,512-518,556-685,756-810,1243-1332`,
  new `crates/sdk/src/client/server_requests.rs`, `crates/sdk/src/events/mod.rs:175-214`
- **Task:** One declarative `server_request_handlers! { name => (method_str, ParamsTy,
  ResponseTy, EventVariant), … }` table (7 rows) expanding to: handler type aliases,
  `Inner` fields + initializers, `set_*`/`clear_*` pairs, `respond_*` wrappers, the
  `try_auto_handle_server_request` dispatch, and `parse_server_request`. Generated names
  must match today's public API exactly.
- **Tests required:** `cargo public-api` diff empty. Add one unit test per dispatch path
  using the existing raw-channel test harness (`client/mod.rs:1385`): a registered handler
  answers the request; an unregistered one publishes the event (pins the current
  fall-through-to-event semantics at `:1330`).
- **Acceptance:** Adding a server request = one table row; a missing row cannot half-land.
- **Risk:** Low. **LOC:** ≈ −245. **Conflicts:** heavy `client/mod.rs` churn — must land
  before 2.5.

### Issue 2.5 — Single RPC method table shared by `CodexClient` and `Codex`
- **Files:** new `crates/sdk/src/protocol/methods.rs`, `crates/sdk/src/client/mod.rs:453-469,
  812-1077`, `crates/sdk/src/api.rs:944-960,1085-1291`
- **Depends on:** Issue 2.4 (avoid conflicting `client/mod.rs` edits).
- **Task:** One callback-style `codex_rpc_table!` listing `(fn_name, "method/string",
  ParamsTy, ResultTy)` for all 43 methods — the only place method-name strings appear.
  `client/mod.rs` invokes it to define `CodexClient` methods; `api.rs` invokes it to define
  the `ensure_initialized`-prefixed `Codex` forwards. Delete the hand-expanded
  `thread_list` (`api.rs:1077-1083`) and define the `skills_remote_read`/`skills_remote_write`
  compat aliases once.
- **Tests required:** `cargo public-api` diff empty (both types keep all methods). Add a
  table-driven test generated from the same macro asserting every method's
  name/params/result triple serializes into a well-formed JSON-RPC request (this gives the
  26 currently-untested methods at least construction coverage). Verify docs.rs-style
  rendering (`cargo doc`) still lists every method.
- **Acceptance:** Adding an RPC = one table row; `Codex` can never silently lag
  `CodexClient` again.
- **Risk:** Low-Medium. **LOC:** ≈ −270.

### Issue 2.6 — Table-ize notification parsing
- **Files:** `crates/sdk/src/events/mod.rs:15-173`, `crates/sdk/src/protocol/notifications.rs:165-179`
- **Task:** One `notification_table! { Variant => ("method/name", PayloadTy), … }`
  expanding into the `ServerNotification` enum, `parse_notification`, and a new
  `method_name()` accessor. Keep the `Unknown { method, params }` fallback. Do **not**
  collapse the 15 opaque variants yet (that is breaking — deferred to Issue 4.6).
- **Tests required:** `protocol_roundtrip.rs` green; add a completeness test iterating the
  table to assert parse→variant→method_name round-trips for every row.
- **Acceptance:** Adding a notification = one table row.
- **Risk:** Low. **LOC:** ≈ −90.

### Issue 2.7 — Deduplicate the transport reader/writer plumbing
- **Files:** `crates/sdk/src/transport/{mod,stdio,ws}.rs`
- **Task:** Extract shared `spawn_writer`/`spawn_reader` helpers (or a
  `spawn_json_channel`) used by both transports; hoist channel capacities
  (`256`/`1024`) to named consts in `transport/mod.rs`; collapse `ws.rs`'s copy-pasted
  `Text`/`Binary` arms (`ws.rs:82-99`) via `Message::into_data()`. Do **not** introduce a
  `Transport` trait — the `TransportHandle` channel-pair seam is good and has no second
  consumer needing dynamics.
- **Tests required:** `integration_stdio` + `integration_ws` green (transport-touching per
  the merge gate). Add a unit test for the reader's invalid-JSON → `InvalidMessage` path
  using an in-memory duplex stream.
- **Acceptance:** One copy of the serialize/send/error-funnel logic; no magic numbers.
- **Risk:** Low. **LOC:** ≈ −50.

### Issue 2.8 — Simplify `pump_turn_events` (dedup arms + one send idiom)
- **Files:** `crates/sdk/src/api.rs:1674-1848`
- **Task:** Hoist the four near-identical delta arms into a `decode_delta` helper + one
  `emit!` macro; unify the two inconsistent send idioms (break-on-error vs. ignore-error)
  to break-on-error; delete the provably-unreachable channel-failure recovery at
  `api.rs:1587-1600` (or convert to `expect` with a comment). Optionally (stretch): split
  into a synchronous `classify(notification) -> PumpAction` + thin async loop so filtering
  logic becomes unit-testable without a tokio channel harness.
- **Tests required:** Existing `pump_turn_events_emits_reasoning_text_deltas`
  (`api.rs:2955`) green. If the classify split is done, add direct unit tests for: guard
  fall-through (notification failing its thread/turn guard is ignored), usage
  accumulation, and terminal-state detection.
- **Acceptance:** One send idiom; delta handling in one place; no silent guard
  fall-through without a test documenting it.
- **Risk:** Low (Medium with the classify split). **LOC:** ≈ −60.

---

## Wave 3 — CLI convergence (spark + agx)

> 3.1 first (it's the foundation crate); 3.2–3.6 then run in parallel.

### Issue 3.1 — Create `crates/cli-core` with the trivial shared helpers
- **Files:** new `crates/cli-core`, root `Cargo.toml`, both CLI crates
- **Task:** New workspace member holding the byte-identical/near-identical helpers:
  `resolve_path_from_file` (spark `:2418-2428` ≡ agx `:298-308`), generic
  `print_chunk`/`ensure_message_separator` (spark's `Write`-generic versions,
  `:1190-1207`; agx adopts them), one `preview(text, max_chars, empty_sentinel)`
  (word-boundary algorithm, replacing spark `crop_preview_text:1102-1124` and agx
  `clipped_text:495-531`), `resolve_codex_home_dir`/`resolve_home_dir`
  (spark `:2272-2294`), `resolve_codex_binary(_with)` (spark `:1142-1188`), and a shared
  `CliError` (thiserror: `Usage`/`Config`/`Runtime`/`Io`/`Client`) with
  `report(err, usage) -> ExitCode`.
- **Tests required:** Move the existing unit tests for each helper into `cli-core`
  (spark's separator tests `:3279-3301`, binary-resolution tests `:3215-3277`, etc.).
  Preview-truncation output changes in spark (sentinel/algorithm) must be listed in the PR.
- **Acceptance:** Both CLIs compile against `cli-core`; zero duplicated helper bodies
  remain (verify by grep for the old function names).
- **Risk:** Low. **LOC:** ≈ −60 net (plus enabling the rest of the wave).

### Issue 3.2 — Port spark's argument parsing to clap
- **Files:** `crates/spark/src/main.rs:27-88` (USAGE), `:110-205` (`CliArgs`),
  `:1209-1848` (`parse_cli_args`), `crates/spark/Cargo.toml`
- **Task:** Replace the 640-line hand-rolled parser (39 mutable accumulators, every flag
  parsed twice for `--x`/`--x=`, a 28-operand `||` expression rejecting exec flags on
  `start`) with clap derive, as agx already uses (`agx:32-56`): subcommands
  `exec`/`start`/`sessions`, `ArgGroup`/`conflicts_with_all` for the conflict rules. Keep
  `parse_cli_args` as a thin adapter returning the existing `ParsedCommand`/`CliArgs` so
  the 30+ parser unit tests keep compiling with minimal edits.
- **Preserve exactly:** `--` prompt passthrough (`:1251-1254`), `-c`/`-r` shorts, prompt
  parts joined with a single space, mutual-exclusion errors the tests assert
  (continue×resume, stdio×ws-url, duplicate flags, start×exec-flags, sessions×prompt),
  and `--final-response`/`--json` behavior. Error text may change format; each changed
  assertion must be updated deliberately, not deleted.
- **Tests required:** All existing `parse_cli_args_*` tests pass (adapted); add tests for
  `--help` exit code and unknown-flag exit code. `integration_spark` green.
- **Acceptance:** No hand-written flag scanning remains; adding a flag = one struct field
  with attributes; USAGE text is generated (or verified consistent with) clap.
- **Risk:** Medium. **LOC:** ≈ −450.

### Issue 3.3 — Shared streaming driver + per-CLI renderers (correctness convergence)
- **Files:** `crates/cli-core` (new module), `crates/spark/src/main.rs:501-632`,
  `crates/agx/src/main.rs:594-663,775-1045`
- **Depends on:** Issue 3.1.
- **Task:** Implement in `cli-core`:
  `trait TurnRenderer { on_item_started/updated/completed, on_agent_delta, on_turn_completed, on_error, … }`
  and `async fn drive_stream(streamed, &mut impl TurnRenderer) -> Result<TurnOutcome, CliError>`
  containing, **once**: agx's per-message-id dedup state machine (`StreamRenderState`,
  agx `:611-637` — spark's single-bool version mis-renders interleaved messages) and
  spark's stream-closed-before-`TurnCompleted` guard (spark `:559-563` — agx currently
  exits silently). Then: spark ships `PlainRenderer` + `JsonRenderer`; agx ships
  `ColorRenderer` (extracting the 212-line `ItemCompleted` match out of `main`, with
  `line(glyph, color, text)`/`dim(text)` helpers collapsing the 47 `if_supports_color`
  closures). Both CLIs get the union of correct behaviors.
- **Tests required:** **This is the wave's key testability win — do it test-first.**
  Renderer unit tests against synthetic `ThreadEvent` vectors: interleaved agent-message
  deltas (two ids), delta-then-completed dedup, turn-failed surfacing, stream-truncation
  error, JSON line shape for at least 5 item variants. These cover ~400 currently
  untestable lines.
- **Acceptance:** One event loop in the workspace; `spark` and `agx` renderers are flat
  per-item mappings with no control flow; both truncation and interleaving behaviors are
  pinned by tests.
- **Risk:** Medium (visible output changes possible — golden-test current output first
  where practical). **LOC:** ≈ −125 net, −3 control-flow copies.

### Issue 3.4 — Derive `Serialize` on `ThreadItem`; delete spark's hand-written JSON mapping
- **Files:** `crates/sdk/src/api.rs:746-942`, `crates/spark/src/main.rs:634-790`
- **Task:** Derive `Serialize` (`#[serde(tag = "type", rename_all = "camelCase")]`, status
  enums via their wire spellings) on `ThreadItem` and member structs in the SDK; delete
  spark's 157-line `thread_item_to_json`. Subsumes Issue 0.6. Coordinate with Issue 4.4
  (serde-derived *De*serialize) — same types; if both are scheduled, do them together.
- **Tests required:** SDK round-trip tests: `parse_thread_item(serde_json::to_value(item))`
  stability for every variant incl. `Unknown` (raw payload preservation). Spark golden
  test for the `--json` line format; PR body documents the casing fix as a contract change.
- **Acceptance:** No hand-written item→JSON code outside serde; `--json` casing consistent.
- **Risk:** Medium (output contract). **LOC:** ≈ −132.

### Issue 3.5 — Decompose `spark::run`; kill the abort boilerplate
- **Files:** `crates/spark/src/main.rs:243-499`
- **Depends on:** Issue 3.2 (CliArgs shape settles first).
- **Task:** Split the 257-line `run` into `run_exec`/`run_start`/`run_sessions` behind a
  10-line dispatcher. Replace the seven copy-pasted
  `match … { Err(e) => { connect_task.abort(); return Err(e) } }` blocks (`:334-394`) with
  `tokio::try_join!(connect_fut, async { build_all_options() })` — preserving the
  connect/config overlap in one line. Extract `build_thread_options`/`build_turn_options`
  from `:396-457`.
- **Tests required:** Behavior-neutral; `integration_spark` green; add a unit test for the
  option-builder extraction (flag → ThreadOptions mapping for a representative set).
- **Acceptance:** `run_exec` ≤ ~80 lines; zero `.abort()` error-plumbing blocks.
- **Risk:** Low-Medium. **LOC:** ≈ −90.

### Issue 3.6 — `spark sessions`: typed preview extraction, bounded fan-out, tests
- **Files:** `crates/spark/src/main.rs:869-1140`, `crates/sdk/src/api.rs`
- **Task:** (a) Move the 183 lines of untyped `serde_json::Value` spelunking
  (`extract_summary_preview`, `extract_item_text`, `item_is_assistant_message`,
  `:958-1140`) into the SDK beside `parse_thread_item`, rewritten against the typed item
  parser instead of guessing 10 key names. (b) Split `list_sessions` into
  `collect_sessions()` / `print_sessions()`. (c) Bound the per-thread `thread_read` N+1
  fan-out (`:899`) with `buffer_unordered(4)` and only when the summary lacks a preview.
- **Tests required:** This subsystem currently has **zero tests**. Add unit tests for
  preview extraction over representative thread payloads (assistant text, summary-only,
  empty) and for `collect_sessions` filtering/sorting with a stubbed client (raw-channel
  harness).
- **Acceptance:** No untyped key-guessing in spark; sessions listing latency no longer
  linear-serialized in thread count.
- **Risk:** Medium (previously untested). **LOC:** ≈ −70 in spark.

### Issue 3.7 — Consolidate spark's flag surface (deprecations)
- **Files:** `crates/spark/src/main.rs`, `README.md`
- **Depends on:** Issue 3.2.
- **Task:** With clap in place: collapse `--sandbox-network-access-enabled|disabled` into
  `--sandbox-network-access <true|false>` (keep old spellings as hidden aliases for one
  release); make `--output-schema-file` a hidden alias of `--output-schema` (exact
  synonyms today, `:1626-1646`); introduce `--transport <ws|ws-no-daemon|stdio>` with
  `--stdio`/`--no-daemon` as hidden aliases; document (README) that `--model-verbosity`,
  `--config-profile`, `--web-search-mode`, `--sandbox-*` sugar flags are equivalent to
  `--config` keys, referencing the Issue 0.5 precedence rule.
- **Tests required:** Alias tests (old spelling still parses, maps to same CliArgs);
  README updated in the same PR.
- **Acceptance:** Documented flag count shrinks; no behavior removed without an alias.
- **Risk:** Low. **LOC:** ≈ −55.

### Issue 3.8 — agx feature-parity essentials: `--cwd`, connect fallback via SDK
- **Files:** `crates/agx/src/main.rs:665-672,729-753`
- **Task:** (a) agx forces `skip_git_repo_check(true)` but never sets
  `working_directory` — add `--cwd` (default: invocation dir) matching spark. (b) Replace
  agx's hand-rolled loopback-prefix matching (`ws://127.0.0.1:`/`ws://localhost:`/
  `ws://0.0.0.0:` string checks, `:738-753`) with the SDK's loopback logic: call
  `start_and_connect_ws` directly for loopback URLs, `connect_ws` otherwise, reusing
  `parse_managed_ws_target` semantics (expose a small `pub fn is_loopback_ws_url` from
  the SDK if needed). Delete the duplicated `DEFAULT_WS_URL` consts in both CLIs
  (spark `:21`, agx `:23`) in favor of `WsConfig::default()`.
- **Tests required:** Unit test for the loopback decision via the SDK helper (the SDK
  already tests `parse_managed_ws_target`, `ws_daemon.rs:412-484`); agx test asserting
  `--cwd` reaches `ThreadOptions`.
- **Acceptance:** One loopback-detection implementation in the workspace; agx runs behave
  deterministically w.r.t. working directory.
- **Risk:** Low-Medium. **LOC:** ≈ −20.

---

## Wave 4 — Architectural / semver-sensitive (sequenced, mostly 0.6)

### Issue 4.1 — Move WS process management out of `client/mod.rs`; break the module cycle
- **Files:** `crates/sdk/src/client/mod.rs:226-427`, `crates/sdk/src/transport/ws_daemon.rs`
- **Task:** `client` currently owns raw `libc::kill` FFI, TCP port probes, and
  `std::thread::sleep` loops while `transport::ws_daemon` imports its own return type back
  out of `client` (`ws_daemon.rs:15`) — a circular dependency. Move `WsServerHandle`,
  `WsStartMode`, and all process/port helpers into the transport layer; re-export from
  `client` to preserve public paths (`lib.rs:23`). Unify the two disagreeing liveness
  checks (`probe_app_server` websocket handshake vs. `TcpListener::bind` probe) on one
  helper. Provide `async fn shutdown()` (replacing the up-to-4s blocking sleep loops that
  currently run on the async runtime, `:294,314,375-379`); `Drop` becomes best-effort
  SIGTERM + `try_wait` only, documented.
- **Tests required:** `integration_ws::start_ws_blocking_returns_owned_handle_and_can_shutdown`
  green (it pins port-release behavior); `cargo public-api` shows only additive changes
  (deprecated sync `shutdown` shim retained).
- **Acceptance:** No process/FFI code in `client/`; no `std::thread::sleep` on async paths;
  one liveness predicate.
- **Risk:** Medium. **LOC:** ≈ −25 plus robustness.

### Issue 4.2 — `ws_daemon` hardening: URL parsing collapse, temp-dir, `spawn_blocking`
- **Files:** `crates/sdk/src/transport/ws_daemon.rs`
- **Depends on:** Issue 4.1 (same file).
- **Task:** (a) Collapse the 107 lines of overlapping URL munging (`parse_managed_ws_target`,
  `parse_ws_url`, `normalized_listen_host`, `log_path_for` — three validators, two IPv6
  formattings) into one `WsTarget::parse(url, Mode)` with a single `Host` switch. (b)
  Replace the hardcoded world-writable `/tmp/codex-app-server-sdk` log dir (`:18`) with
  `std::env::temp_dir()`-based, `0o700` on unix, overridable via `WsStartConfig`; open
  logs without following symlinks. (c) Replace `spawn_daemon_launcher_thread`
  (`:276-303` — an OS thread + `std::sync::mpsc` ack reimplementing `spawn_blocking`)
  with `tokio::task::spawn_blocking` around `std::process::Command` (keep
  `std::process`, not `tokio::process`, so the detached daemon isn't reaped). (d) Key
  `STARTUP_LOCK` by `(host, port)` instead of one global mutex, or document the
  serialization. (e) Decide Windows support explicitly: currently `process_group` and the
  kill helpers are unix-only no-ops elsewhere, so daemon shutdown on Windows silently
  half-works — either `#[cfg(unix)]`-gate the daemon module with a clear compile error or
  implement Job Objects; pick one and document it.
- **Tests required:** Existing URL unit tests (`ws_daemon.rs:406-490`) migrated to
  `WsTarget::parse`; `integration_ws` green; new unit test for log-path derivation incl.
  IPv6.
- **Acceptance:** One URL parser, one host formatter; no fixed `/tmp` path; no bespoke
  thread+ack machinery.
- **Risk:** Low-Medium (security-relevant improvements). **LOC:** ≈ −60.

### Issue 4.3 — Unify the initialize/ready state machine
- **Files:** `crates/sdk/src/client/mod.rs:433-434,687-754,1085-1147`,
  `crates/sdk/src/api.rs:970-971,1327-1346`
- **Task:** The handshake state is tracked by three flags across two layers
  (`Inner.initialized` + `Inner.ready` + `CodexInner.initialized` + a mutex), and
  `Codex::ensure_initialized` swallows `Err(AlreadyInitialized)` (`api.rs:1339`) precisely
  because the layers race. Replace with one `AtomicU8` state enum
  (`New/Initializing/Initialized/Ready`) in `Inner`; expose idempotent
  `CodexClient::ensure_ready()`; `Codex::ensure_initialized` becomes a one-line
  delegation; remove the duplicate flag+mutex from `CodexInner`; hoist the
  `"initialize"`/`"initialized"` method-name string comparisons to constants used once.
- **Tests required:** `integration_stdio` + `integration_api_stdio` green (they exercise
  not-ready and already-initialized paths). Add a concurrency unit test: two tasks racing
  `ensure_ready` on a raw-channel client, exactly one initialize is sent.
- **Acceptance:** One state variable; no error-swallowing match; racing initializers safe
  by construction.
- **Risk:** Medium. **LOC:** ≈ −25.

### Issue 4.4 — Replace `parse_thread_item` with serde-derived wire types
- **Files:** `crates/sdk/src/api.rs:2262-2660`, new `crates/sdk/src/protocol/items.rs`
- **Coordinate with:** Issue 3.4 (same types; ideally one PR pair).
- **Task:** The wire shape of thread items lives implicitly inside a 247-line, ~80-branch
  hand-rolled `parse_thread_item` (the crate's largest function; the expression
  `object.get(k).and_then(Value::as_str).unwrap_or_default().to_string()` appears 28
  times) plus 150 lines of satellite parsers. Define the wire shape declaratively in
  `protocol/items.rs` (`#[serde(tag = "type", rename_all = "camelCase")]`, `serde(default)`
  to reproduce the forgiving missing-field semantics, `#[serde(alias = "in_progress")]`
  for status spellings, untagged-or-custom fallback preserving `UnknownItem { raw }`).
  `ThreadItem` becomes the derived type or a thin `From`.
  Fallback plan if the derive route stalls on fidelity: land the `s(object, "key")`
  helper collapse instead (−60 LOC, zero behavior change) and re-scope.
- **Tests required:** Existing pinning tests green
  (`parse_missing_documented_thread_item_variants` `api.rs:2815`,
  `parse_unknown_item_preserves_payload` `api.rs:3082`). Add a corpus round-trip test:
  for each variant, a canned wire JSON → parse → (with 3.4) serialize → parse is stable.
  Add malformed-input cases (missing fields, unknown status strings) asserting the
  forgiving behavior is preserved exactly.
- **Acceptance:** No hand-rolled field extraction for items; wire schema readable in one
  declarative file; unknown-variant passthrough intact.
- **Risk:** Medium. **LOC:** ≈ −120 net (plus enabling 3.4's −132).

### Issue 4.5 — Promote the ten `extra`-only options to typed protocol fields
- **Files:** `crates/sdk/src/protocol/requests.rs`, `crates/sdk/src/api.rs`
- **Depends on:** Issues 2.1, 2.3.
- **Task:** `skip_git_repo_check`, `web_search_mode`, `web_search_enabled`,
  `network_access_enabled`, `additional_directories`, `collaboration_mode`, `config`,
  `dynamic_tools`, `experimental_raw_events`, `persist_extended_history` are typed in the
  API layer but travel as hand-inserted camelCase string keys in `extra`. Add them as
  typed `#[serde(skip_serializing_if = "Option::is_none", rename = "…")]` fields on
  `ThreadStartParams`/`ThreadResumeParams`/`TurnStartParams` (exact same wire keys). The
  `build_*_params` functions become near-trivial struct literals; the Issue 2.3 inserter
  shrinks to nothing. Where enums exist (Issue 2.1), use them with `#[serde(other)]`-style
  tolerance so unknown server values don't hard-fail.
- **Tests required:** The Issue 2.3 payload snapshots must remain byte-identical (this is
  the whole point — pure representation change). `protocol_roundtrip.rs` extended for the
  new fields incl. `extra`-flatten coexistence.
- **Acceptance:** No string-key insertions for known options; typo-class bugs impossible
  for these fields.
- **Risk:** Medium (wire fidelity). **LOC:** ≈ −90 net.

### Issue 4.6 — 0.6 breaking batch (single coordinated release PR)
- **Files:** multiple
- **Task:** Batch the small breaking changes agreed above into one 0.6 release:
  - Remove items deprecated in Issues 1.3/4.1 (`start_ws`, `JsonRpcNotification/Response`,
    sync `shutdown`).
  - Move `env` off `WsConfig` into the start-owning config (`WsStartConfig`), fixing the
    B2 trap structurally.
  - Split the overloaded `ClientError::TransportSend` (36 construction sites spanning
    URL validation, connect, send, receive, spawn, readiness-timeout, port-conflict) into
    `Config`/`Startup{source, log_path}`/genuine transport variants; hoist the inline RPC
    codes `-32001`/`-32098` (`client/mod.rs:1345,1365`) to named consts. Update the
    substring-matching assertions in `ws_daemon.rs:449-472` and
    `tests/integration_ws.rs:231-256` to match on variants, not strings.
  - Collapse the 15 structurally-identical opaque notification variants into
    `Opaque { kind, extra }` (or keep — decide with maintainer; document either way).
  - `schema.rs` trim: drop `serialize_openai_value`/`deserialize_openai_value` and the
    two unused trait defaults (`to_openai_value`/`from_openai_value`) — pure aliases of
    serde_json with zero external callers. Keep `openai_json_schema_for` and the derive
    (documented in README); the blanket-impl replacement of the proc-macro derive is
    explicitly deferred to a 1.0 discussion.
  - Rename `api::Turn` → `api::TurnResult` with a deprecated alias (collides with
    `responses::Turn` today).
- **Tests required:** Full workspace + all four live suites; `cargo public-api` diff
  reviewed as the release checklist; CHANGELOG entries per removal with migration notes.
- **Acceptance:** One migration document; no string-matching on error messages anywhere
  in tests.
- **Risk:** High (coordinated). **LOC:** ≈ −120.

### Issue 4.7 — Unify the two agent-config systems (design note first)
- **Files:** `crates/spark/src/main.rs:2261-2428`, `crates/agx/src/main.rs:99-493`,
  `crates/cli-core`
- **Depends on:** Issue 3.1. **Do last in this wave.**
- **Task:** Today `--agent reviewer` resolves against `~/.codex/config.toml [agents.*]` +
  role files in spark (yielding *only* an instructions string — model/sandbox fields are
  parsed and discarded) but against `.codex/agents/*.toml` project-upward search in agx
  (yielding 12 fields + config map). Two schemas, two search orders, two error formats,
  one flag name. Write a short ADR first (adrs/0005) choosing the target model —
  recommendation: agx's richer schema and search path as canonical, with a
  legacy-compat reader for spark's `config.toml [agents.*]` — then implement it once in
  `cli-core` and delete both copies. The near-identical inner precedence functions
  (spark `resolve_agent_instructions:2362-2416` ≡ agx `resolve_developer_instructions:248-296`)
  can be extracted early as a low-risk first commit.
- **Tests required:** Port both crates' existing agent-loading test suites (spark
  `:3030-3213`, agx's 11 tests) against the unified module; add explicit tests for the
  legacy-compat path and for precedence when both project and user files exist.
- **Acceptance:** `--agent` means the same thing in both binaries; spark stops silently
  discarding role-file fields; one "agent not found" message format.
- **Risk:** High (user-visible config behavior). **LOC:** ≈ neutral, −1 subsystem.

---

## 5. Cross-cutting quality requirements (apply to every issue)

1. **Merge gate:** `cargo fmt --all` clean, `cargo check --workspace`,
   `cargo test --workspace -- --nocapture`; the four live integration suites for any
   protocol/lifecycle/transport-touching change (see `AGENTS.md` — live-test failures are
   actionable, not environmental).
2. **Public API discipline:** any PR touching `crates/sdk` public items attaches a
   `cargo public-api`/`cargo doc` diff and states "additive / deprecation / breaking".
   Breaking goes only through Issue 4.6.
3. **Test-first for untested regions:** where an issue notes a region has no tests
   (spark streaming/JSON output, `build_thread_config`, sessions listing, agx main loop),
   pin current behavior with tests in a first commit, then refactor.
4. **No drive-by scope:** each issue is one PR; conflicts noted per issue determine
   ordering within a wave.
5. **Behavior changes are announced:** any user-visible change (flag alias, output
   casing, error text, precedence) is called out in the PR body and README where relevant.

## 6. Expected end-state summary

| Area | Now | After |
|---|---|---|
| `client/mod.rs` | 1,461 lines, ~62% repeated blocks | ~700 lines, table-driven |
| `api.rs` | 3,442 lines, 3 config styles, hand parsers | ~2,400 lines, 1 style, derived parsing |
| `spark/main.rs` | 3,314 lines, hand-rolled CLI parser | ~1,900 lines, clap, shared renderers |
| `agx/main.rs` | 1,561 lines, 7-level nesting in `main` | ~1,000 lines, shared renderers |
| Shared CLI code | 0 lines | `cli-core` ~600 lines, single-tested |
| Silent-drift surfaces | 4 (RPC table ×2, handlers ×5, enums ×3, CLI forks) | 0 (all table-driven) |
| Known bugs | 6 (B1–B6) | 0 |
| Untested critical paths | RPC surface 60%, spark output 100%, sessions 100% | construction-covered + renderer/golden tests |
