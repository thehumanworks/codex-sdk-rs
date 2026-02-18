# Spark Session Resumption

`spark` supports resuming prior sessions so follow-up prompts can reuse thread context.
By default, `spark` connects over websocket at `ws://127.0.0.1:4222`.
Use `--stdio` to force app-server stdio transport.

## Flags

- `-c`, `--continue`
  - Resume the most recent recorded session.
  - This maps to the same user intent as `codex resume --last`.
- `-r <session_id>`, `--resume <session_id>`
  - Resume a specific session/thread id.

Only one of `--continue` or `--resume` can be provided per invocation.

## Examples

Start a fresh spark session:

```bash
cargo run --bin spark -- "Remember that my codename is atlas."
```

Start a fresh session over stdio:

```bash
cargo run --bin spark -- --stdio "Remember that my codename is atlas."
```

Continue the latest spark session:

```bash
cargo run --bin spark -- --continue "What codename did I give you?"
```

Resume an explicit session id:

```bash
cargo run --bin spark -- --resume thread_123 "Summarize our last decision."
```

Use final-response-only mode with resume:

```bash
cargo run --bin spark -- --final-response --resume thread_123 "Give me the final answer only."
```

## Notes

- `--continue` fails with a clear error when no recorded sessions exist yet.
- `--resume` fails if the supplied session id does not exist or cannot be loaded.
