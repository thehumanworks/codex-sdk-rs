# ADR 0001: Separate `spark` CLI From the SDK Crate

- Status: Accepted
- Date: 2026-02-20

## Context

The repository previously bundled the SDK library and `spark` CLI binary in one crate (`src/lib.rs` plus `src/bin/spark.rs`).

That layout made dependency direction ambiguous and coupled CLI concerns (argument parsing, profile loading, CLI-only tests) to SDK packaging. The intended architecture is one-way:

- `spark` depends on `codex-app-server-sdk`
- `codex-app-server-sdk` must not depend on `spark`

## Decision

Adopt a Cargo workspace with separate crates:

- `crates/sdk` contains the `codex-app-server-sdk` library crate.
- `crates/spark` contains the `spark` binary crate.
- `codex-app-server-sdk-macros` remains a workspace member consumed by `crates/sdk`.

Implementation details:

- Move SDK source, examples, and SDK tests under `crates/sdk/`.
- Move `spark` entrypoint to `crates/spark/src/main.rs`.
- Move spark integration tests to `crates/spark/tests/`.
- Convert root `Cargo.toml` to a virtual workspace manifest.

## Consequences

Positive:

- Dependency direction is explicit and enforceable by Cargo.
- SDK release surface is cleaner (library-focused, no bundled CLI target).
- Spark CLI can evolve independently (flags, transport defaults, CLI-specific tests).
- Workspace commands can target each package directly (`-p codex-app-server-sdk`, `-p spark`).

Tradeoffs:

- Commands and docs must use package-qualified invocations in multi-package contexts.
- Some test and install scripts require updated paths.

## Notes

- This ADR is structural only; no protocol behavior change is intended.
- Future CLI tools should follow the same pattern: separate crate, SDK as dependency only.
