# codex-app-server-sdk-macros

Companion proc-macro crate for [`codex-app-server-sdk`](https://docs.rs/codex-app-server-sdk).

This crate provides the `#[derive(OpenAiSerializable)]` derive used by the SDK's typed structured-output helpers.

Most consumers should depend on `codex-app-server-sdk` directly and import the derive from there:

```rust
use codex_app_server_sdk::OpenAiSerializable;
```
