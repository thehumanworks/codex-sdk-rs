# codex-app-server-sdk-macros

Companion proc-macro crate for [`codex-app-server-sdk`](https://docs.rs/codex-app-server-sdk).

This crate provides:

- `#[derive(OpenAiSerializable)]` for the SDK's typed structured-output helpers.
- `#[openai_type]` as a convenience attribute that appends the SDK-owned `serde`, `schemars`, and `OpenAiSerializable` derives plus the required crate-path overrides.

Most consumers should depend on `codex-app-server-sdk` directly and import the derive from there:

```rust
use codex_app_server_sdk::OpenAiSerializable;
```
