# codex-app-server-sdk

Tokio Rust SDK for Codex App Server JSON-RPC over JSONL.

## Status

- `0.1.0`
- Focused on deterministic automation: explicit timeouts and no implicit retries.
- Typed v2 request methods with raw JSON fallback for forward compatibility.

## Features

- `stdio` (default): spawn `codex app-server` locally.
- `ws`: websocket transport with loopback daemon management.
  - For loopback URLs (`ws://127.0.0.1:*`, `ws://[::1]:*`, `ws://localhost:*`), the SDK reuses an existing app-server or auto-starts `codex app-server --listen ...` and leaves it running.
  - Non-loopback URLs remain connect-only (no process management).
  - Daemon logs are written to `/tmp/codex-app-server-sdk/*.log`.

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

## Quickstart (high-level typed abstractions)

```rust
use codex_app_server_sdk::abstractions::{Codex, ThreadOptions, TurnOptions};
use codex_app_server_sdk::StdioConfig;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
let mut thread = codex.start_thread(ThreadOptions::default());

let turn = thread
    .run("Summarize this repository in two bullet points.", TurnOptions::default())
    .await?;

println!("thread: {}", thread.id().unwrap_or("<unknown>"));
println!("response: {}", turn.final_response);
# Ok(())
# }
```

Use `run_streamed(...)` when you need incremental item and lifecycle events.

## Quickstart (ws, persistent loopback daemon)

```rust
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};
use codex_app_server_sdk::requests::{ClientInfo, InitializeParams};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = CodexClient::connect_ws(WsConfig {
    url: "ws://127.0.0.1:4222".to_string(),
    options: ClientOptions::default(),
}).await?;

let init = InitializeParams::new(ClientInfo::new("my_client", "My Client", "0.1.0"));
let _ = client.initialize(init).await?;
client.initialized().await?;
# Ok(())
# }
```

## Reliability model

- No automatic retries for any RPC method.
- Every request has a timeout (`ClientOptions::default_timeout`) with per-call override available through raw request APIs.
- Requests are blocked client-side until you complete both steps: `initialize()` then `initialized()`.
- Unknown events and fields are preserved through `Unknown` variants and `extra` maps.

## Compatibility

The SDK checks local `codex-cli` version (stdio mode) against the tested range:

- `>=0.100.0-alpha.2, <0.101.0`

Control behavior with `CompatibilityPolicy`:

- `Warn` (default): continue and emit compatibility warning event.
- `Strict`: fail client startup on mismatch.
- `Off`: skip checks.

## Auth support

Typed methods include:

- `account/read`
- `account/login/start`
- `account/login/cancel`
- `account/logout`
- `account/rateLimits/read`

Server-initiated `account/chatgptAuthTokens/refresh` is surfaced as typed events, with optional automatic handler registration.

## Raw fallback

Use:

- `send_raw_request(method, params, timeout)`
- `send_raw_notification(method, params)`

for newly added methods or fields not yet wrapped in typed helpers.

## Examples

- `examples/turn_start_stream.rs`
- `examples/auth_api_key.rs`
- `examples/raw_fallback.rs`
- `examples/ws_persistent.rs`
- `examples/high_level_run.rs`
- `examples/high_level_streamed.rs`

## Integration tests

These tests execute against a real local `codex app-server` process:

```bash
cargo test --test integration_stdio -- --ignored --nocapture
cargo test --test integration_abstractions_stdio -- --ignored --nocapture
cargo test --features ws --test integration_ws -- --ignored --nocapture
```

## License

MIT
