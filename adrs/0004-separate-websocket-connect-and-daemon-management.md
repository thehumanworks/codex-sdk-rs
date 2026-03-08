# 2. Separate WebSocket connection and daemon management

Date: 2025-03-07

## Status

Accepted

## Context

The SDK previously automatically managed a local loopback `codex app-server` daemon whenever connecting to a loopback WebSocket URL using `CodexClient::connect_ws`. This behavior was convenient for local usage but prevented the SDK and the `spark` CLI from easily connecting to an existing WebSocket server running on a loopback URL without attempting to probe and potentially spawn a new daemon process.

## Decision

We separate the functionality of connecting to a WebSocket server from the functionality of managing a local `codex app-server` daemon:

1. `CodexClient::connect_ws` and `Codex::connect_ws` now strictly establish a connection to the provided WebSocket URL.
2. We introduced `CodexClient::start_and_connect_ws` and `Codex::start_and_connect_ws`, which perform the daemon auto-start and management logic before connecting.
3. The `spark` CLI retains the daemon management behavior for local loopback URLs by default, but now supports a `--no-daemon` flag. When provided, the CLI will use `connect_ws` to connect to any WebSocket URL (loopback or publicly accessible) without attempting to start or manage a daemon process.

## Consequences

- The high-level APIs are explicit about when they manage local processes versus when they only attempt a network connection.
- Users relying on `connect_ws` to auto-start local servers must migrate to `start_and_connect_ws`.
- The `spark` CLI is now more flexible, allowing connections to already-running loopback instances via the `--no-daemon` flag or natively connecting to publicly accessible external URLs.
