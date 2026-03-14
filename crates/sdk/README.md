# codex-app-server-sdk

Tokio Rust SDK for Codex App Server JSON-RPC over JSONL.

## Status

- `0.2.0`
- Focused on deterministic automation: explicit timeouts and no implicit retries.
- Typed v2 request methods with raw JSON fallback for protocol drift.

## Features

- `stdio`: spawn `codex app-server` locally.
- `ws`: websocket transport with loopback daemon management.
  - For loopback URLs (`ws://127.0.0.1:*`, `ws://[::1]:*`, `ws://localhost:*`), `start_and_connect_ws` reuses an existing app-server or auto-starts `codex app-server --listen ...` and leaves it running.
  - Use `connect_ws` to connect directly without any process management.
  - Daemon logs are written to `/tmp/codex-app-server-sdk/*.log`.
- High-level typed thread helpers:
  - `Codex::ask(...)`
  - `Codex::ask_with_options(...)`
  - `ResumeThread` enum for typed resume targets
  - `Codex::resume_thread_by_id(...)`
  - `Codex::resume_latest_thread(...)`
  - `Thread::run(...)`
  - `Thread::run_streamed(...)`
- Typed schema generation via `OpenAiSerializable` and `openai_json_schema_for::<T>()`.

## Requirements

- `codex` CLI installed and available on `PATH`.
- `codex app-server` must start locally for live flows.
- Active Codex authentication for account/model/turn requests.

## Quickstart (stdio)

```rust
use codex_app_server_sdk::{CodexClient, StdioConfig};
use codex_app_server_sdk::requests::{ClientInfo, InitializeParams, ThreadStartParams, TurnStartParams};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;

let init = InitializeParams::new(ClientInfo::new("my_client", "My Client", "0.1.0"));
let _ = client.initialize(init).await?;
client.initialized().await?;

let thread = client.thread_start(ThreadStartParams::default()).await?;
let thread_id = thread.thread.id;

let turn = client
    .turn_start(TurnStartParams::text(thread_id, "Summarize this repository."))
    .await?;

println!("turn: {}", turn.turn.id);
# Ok(())
# }
```

## Quickstart (high-level typed API)

```rust
use codex_app_server_sdk::api::{
    Codex, ModelReasoningEffort, SandboxMode, ThreadOptions, TurnOptions, WebSearchMode,
};
use codex_app_server_sdk::StdioConfig;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
let thread_options = ThreadOptions::builder()
    .sandbox_mode(SandboxMode::WorkspaceWrite)
    .model_reasoning_effort(ModelReasoningEffort::Medium)
    .web_search_mode(WebSearchMode::Live)
    .skip_git_repo_check(true)
    .build();
let mut thread = codex.start_thread(thread_options);

let turn = thread
    .run(
        "Summarize this repository in two bullet points.",
        TurnOptions::default(),
    )
    .await?;

println!("thread: {}", thread.id().unwrap_or("<unknown>"));
println!("response: {}", turn.final_response);
# Ok(())
# }
```

Use `run_streamed(...)` when you need incremental item and lifecycle events.

Resume a recorded thread with a typed resume target:

```rust
# use codex_app_server_sdk::api::{Codex, ResumeThread, ThreadOptions};
# use codex_app_server_sdk::StdioConfig;
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
let mut thread = codex.resume_thread(ResumeThread::ById("thread_123".to_string()), ThreadOptions::default());
# Ok(())
# }
```

Resume the latest recorded thread for a workspace:

```rust
# use codex_app_server_sdk::api::{Codex, ResumeThread, ThreadOptions};
# use codex_app_server_sdk::StdioConfig;
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
let mut thread = codex.resume_thread(
    ResumeThread::Latest,
    ThreadOptions::builder()
        .working_directory("/path/to/project")
        .build(),
);
# Ok(())
# }
```

The explicit helpers remain available: use `resume_thread_by_id(...)` and `resume_latest_thread(...)` when you prefer the narrower API surface.

`AgentMessageItem.phase` mirrors the app-server's optional `agentMessage.phase` field (`commentary` or `final_answer`). Use `message.is_final_answer()` to identify the final turn message from `ItemCompleted`; `Turn.final_response` and `ask(...)` already prefer the `final_answer` item when the server provides it and otherwise fall back to the last completed agent message.

## Typed output schema

```rust
use codex_app_server_sdk::api::{Codex, ThreadOptions, TurnOptions};
use codex_app_server_sdk::{JsonSchema, OpenAiSerializable, StdioConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema, OpenAiSerializable)]
struct Reply {
    answer: String,
}

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
let mut thread = codex.start_thread(ThreadOptions::default());
let turn_options = TurnOptions::builder().output_schema_for::<Reply>().build();
let turn = thread
    .run("Respond with JSON only and include the `answer` field.", turn_options)
    .await?;

let value: serde_json::Value = serde_json::from_str(&turn.final_response)?;
let reply = Reply::from_openai_value(value)?;
println!("{}", reply.answer);
# Ok(())
# }
```

Use `codex_app_server_sdk::JsonSchema` instead of adding a separate `schemars` dependency unless you deliberately need a different version elsewhere in your application. That keeps the derive macro and `OpenAiSerializable` on the same trait version.

If you want the SDK to wire the derives and crate paths for you, use the convenience attribute:

```rust
#[codex_app_server_sdk::openai_type]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reply {
    #[serde(rename = "final_answer")]
    answer: String,
}
```

## Websocket flow

```rust
use codex_app_server_sdk::api::{ThreadOptions, TurnOptions};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};
use std::collections::HashMap;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = CodexClient::start_and_connect_ws(WsConfig {
    url: "ws://127.0.0.1:4222".to_string(),
    env: HashMap::new(),
    options: ClientOptions::default(),
}).await?;

let mut thread = client.start_thread(ThreadOptions::default());
let turn = thread
    .run("Reply with exactly: ok", TurnOptions::default())
    .await?;

println!("response: {}", turn.final_response);
# Ok(())
# }
```

## Reliability model

- No automatic retries for any RPC method.
- Every request has a timeout (`ClientOptions::default_timeout`) with per-call override available through raw request APIs.
- Requests are blocked client-side until you complete both steps: `initialize()` then `initialized()`.
- Unknown events and fields are preserved through `Unknown` variants and `extra` maps.

## Raw fallback

Use:

- `send_raw_request(method, params, timeout)`
- `send_raw_notification(method, params)`

for newly added methods or fields not yet wrapped in typed helpers.

## Examples

- `cargo run -p codex-app-server-sdk --example turn_start_stream`
- `cargo run -p codex-app-server-sdk --example raw_fallback`
- `cargo run -p codex-app-server-sdk --example high_level_run`
- `cargo run -p codex-app-server-sdk --example high_level_streamed`
- `cargo run -p codex-app-server-sdk --example high_level_resume`
- `cargo run -p codex-app-server-sdk --example high_level_output_schema`
- `cargo run -p codex-app-server-sdk --example ws_persistent`
- `cargo run -p codex-app-server-sdk --example web_search_agent`
