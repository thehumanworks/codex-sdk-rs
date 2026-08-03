# Luna

Luna is an opinionated interactive and one-shot CLI for the Codex app-server. It
defaults to the `gpt-5.6-luna` model, maximum reasoning effort, and a reusable
local WebSocket app-server.

## Install a release

Luna requires the Codex CLI to be installed and authenticated, but it does not
install Codex, request credentials, or bundle the app-server.

The version-pinned installer supports macOS arm64/x86_64 and Linux x86_64 with
glibc. It downloads one immutable archive, verifies it against the release's
`SHA256SUMS`, and installs only the `luna` executable:

```sh
curl -fsSL "https://raw.githubusercontent.com/thehumanworks/codex-sdk-rs/luna-v<VERSION>/scripts/install-luna.sh" \
  | sh -s -- --version "<VERSION>"
```

For a reviewable installation, download `scripts/install-luna.sh` from the same
version tag, inspect it, and run:

```sh
sh install-luna.sh --version "<VERSION>" --to "$HOME/.local/bin"
```

The installer rejects mutable `latest` versions and checksum mismatches. Release
archives, `SHA256SUMS`, `release-manifest.json`, and GitHub build-provenance
attestations are attached to each `luna-v*` release. Verify an attestation with:

```sh
gh attestation verify "luna-v<VERSION>-<TARGET>.tar.gz" \
  --repo thehumanworks/codex-sdk-rs
```

Then diagnose the local installation without a model request or upstream network
probe:

```sh
luna doctor --summary
```

Use `luna doctor --live` when you want Luna to run the upstream Codex doctor,
connect to the selected app-server, complete the protocol handshake, and check
the account state. Doctor output never includes credential values or full local
paths; WebSocket credentials and query values are redacted.

## Run

With no URL option or environment override, `exec` probes
`ws://127.0.0.1:4222` and automatically starts `codex app-server --listen ...`
when no server is running:

```sh
luna exec "Summarize this repository in one sentence."
```

The WebSocket URL precedence is:

1. `--ws-url`
2. `CODEX_APP_SERVER_WS_URL`
3. legacy `CODEX_WEB_SERVER_URL`
4. `ws://127.0.0.1:4222`

Explicit URLs are connect-only: Luna will not start or own a server supplied by
flag or environment. `luna start [--ws-url URL]` explicitly reuses or starts a
loopback server. `--no-daemon` also forces connect-only behavior, and `--stdio`
spawns one app-server process owned by the current command.

Set `CODEX_BINARY` to an explicit executable path when Codex is not discoverable
on `PATH`:

```sh
CODEX_BINARY=/opt/codex/bin/codex luna doctor --summary
```

Other common flows:

```sh
luna exec --fast "Use the fast service tier for this turn."
luna exec --final-response "Summarize this repository."
luna exec --continue "Follow up on the previous answer."
luna exec --resume <SESSION_ID> "Continue this session."
luna sessions
luna sessions --all
```

## Chat

`luna chat` opens a multi-turn Ratatui interface with a monochrome lunar theme,
streaming activity, a persistent moon splash, history, scrolling, and inline
completion:

```sh
luna chat
luna chat --continue
luna chat --stdio --model gpt-5.6-luna
luna chat "Begin by summarizing this repository."
```

`chat` and `exec` share the exact same argument definition, including transport,
session, model, reasoning, policy, config, schema, and optional initial-prompt
arguments. In chat, `--final-response` hides intermediate activity and `--json`
shows the typed event JSON inside the transcript. Chat requires interactive stdin
and stdout; use `luna exec` for pipes and scripts.

Type `/` to autocomplete host commands. `/compact` requests context compaction;
`/effort [level]` and `/model [name]` inspect or change later turns; `/skills`
lists enabled Codex skills; `/skill <name>` or a leading `$` completes a skill
mention. `/help`, `/clear`, and `/quit` are also available. Tab accepts the ghost
suggestion, Up/Down selects suggestions or prompt history, PgUp/PgDn scrolls, and
Ctrl-C interrupts a running turn.

Run `luna --help` for the complete typed thread/turn option surface.
Without `--fast`, Luna explicitly selects the `default` service tier. With
`--fast`, it sends the app-server `serviceTier: "fast"` override for new,
resumed, and subsequent turns over either transport.
Generate shell completions from that same declarative command definition:

```sh
luna completions zsh > "${fpath[1]}/_luna"
luna completions bash > "$HOME/.local/share/bash-completion/completions/luna"
```

## Diagnostic and error contracts

`luna doctor --json` emits `DoctorReportV1`, identified by
`"schema_version": 1`. Each check contains a stable code, status, summary,
redaction marker, and optional next action. Offline doctor does not make model
requests or run the upstream network checks.

Command failures print a stable category and use these exit codes:

| Exit | Category | Meaning |
|---:|---|---|
| 1 | `luna.io` or failed doctor | Generic I/O failure or one or more failed checks |
| 2 | `luna.usage` | Invalid command-line input |
| 3 | `luna.dependency` | Missing or unusable Codex dependency |
| 4 | `luna.configuration` | Invalid or conflicting configuration |
| 5 | `luna.authentication` | App-server reports no authenticated account |
| 6 | `luna.transport` | Process, WebSocket, timeout, or connection failure |
| 7 | `luna.protocol` | Handshake, RPC, or response-shape failure |
| 8 | `luna.turn` | Turn execution failure |

## Compatibility

Support is earned by the tagged release workflow, not inferred from successful
Rust compilation.

| Target | Archive | Offline doctor | stdio | Managed WebSocket | Status |
|---|---|---|---|---|---|
| macOS arm64 | `aarch64-apple-darwin` | Required | Required | Required | Supported after release gates pass; archives are currently unsigned |
| macOS x86_64 | `x86_64-apple-darwin` | Required | Required | Required | Supported after release gates pass; archives are currently unsigned |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | Required | Required | Required | Supported with the declared Ubuntu 22.04 glibc baseline |
| Windows x86_64 | None | Unit-tested path rules only | Experimental from source | Unsupported | Not advertised; managed daemons fail with an actionable stdio/connect-only fallback |

macOS users may need to approve an unsigned downloaded binary according to their
organization's Gatekeeper policy. Luna does not claim signing or notarization
until those gates are present in the release workflow.

## Build from source

From the workspace root:

```sh
cargo install --path crates/luna --bin luna --force --root "$HOME/.local"
cargo test -p luna --bin luna -- --nocapture
cargo test -p luna --test integration_luna -- --nocapture
```

Release tags use the form `luna-v<VERSION>`. The release workflow builds native
macOS arm64/x86_64 and Ubuntu 22.04 x86_64 archives, smoke-tests the binary and
missing-Codex doctor behavior, publishes checksums and a build manifest, and
creates GitHub provenance attestations.

The parser, help, conflicts, aliases, and completion output share a single `clap`
definition in `cli.rs`. Subcommands live under `commands/`; config layering,
session discovery, websocket resolution, and human/JSON output each have focused
modules so they can be tested without a live model turn.

Human output includes agent text, reasoning summaries, plans, tool calls,
commands, patches, status items, and errors. Luna uses `owo-colors` for semantic
styling and emits no ANSI escapes when stdout is not a terminal or `NO_COLOR` is
set. It does not echo the submitted user message or print reasoning items with no
visible text. `--json` retains the complete newline-delimited event schema for
machine consumers.
