# Project Guidance

## Overview

PunchHole is a Rust 2024 command-line service for maintaining multiple direct IPv4 TCP mappings behind a full-cone/NAT1 router. It uses the same local port for an HTTP keepalive connection, a TCP STUN request, and the forwarding listener. It is direct-only: there is no TURN, relay, UDP/uTP forwarding, CGNAT support, authentication, or ACL.

## Repository map

- `src/main.rs`: minimal binary entry point.
- `src/lib.rs`: module wiring, tracing initialization, and public test-facing re-exports.
- `src/cli.rs`: `clap` command-line definition.
- `src/config.rs`: strict JSON/inline configuration parsing and mapping validation.
- `src/net.rs`: reusable bound sockets and Linux accept-error classification.
- `src/http.rs`: persistent HTTP `HEAD` setup, strict response parsing, and keepalive loop.
- `src/stun.rs`: TCP STUN request framing and strict IPv4 `XOR-MAPPED-ADDRESS` parsing.
- `src/forward.rs`: mapping supervision, listeners, client limits, and bidirectional forwarding.
- `src/notify.rs`: notification queue, retries, coalescing, script timeout, and Linux process-group cleanup.
- `tests/`: integration tests grouped by module. Keep tests here; do not add inline test modules under `src/`.
- `example.config.json`: strict JSON example using documentation-only endpoints and an absolute placeholder script path.
- `scripts/qbittorrent-set-port.sh`: POSIX shell helper for qBittorrent WebAPI port updates.

## Required commands

Use the lockfile for reproducible commands:

```sh
cargo build --release --locked
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt -- --check
```

Before declaring any task complete, `cargo clippy-fix` is mandatory and must emit no warning or error:

```sh
cargo clippy-fix
```

Inspect any automatic fixes for networking, ownership, timeout, and lock-scope changes, then rerun formatting, tests, and strict Clippy. Do not hide failures with broad lint allowances. If the shell helper changes, also run:

```sh
sh -n scripts/qbittorrent-set-port.sh
```

## Coding and testing conventions

- Follow `rustfmt`; keep `main.rs` thin and changes within the existing concern-based modules.
- Use structured `tracing` fields and levels for runtime diagnostics. Do not add `println!` or `eprintln!` logging.
- Preserve explicit timeouts, retry paths, shutdown behavior, and checked protocol framing at all network boundaries.
- Prefer small focused changes and existing standard-library or installed-crate facilities; avoid speculative abstractions and dependencies.
- Add focused integration tests in the matching `tests/*.rs` file. Tests use loopback sockets, ephemeral ports, and temporary files; they must not mutate a real qBittorrent instance.
- Keep `Cargo.lock` updated through Cargo, never by hand.
- Do not use emoji in source, comments, documentation, tests, scripts, configuration, or user-facing text.

## Protocol and behavior invariants

- HTTP, STUN, and target endpoints accept IPv4 literals or DNS hostnames with a port. Hostnames resolve once at configuration load to the first IPv4 result; runtime networking remains IPv4-only. Local mapping ports are nonzero and unique.
- `net::new_bound_socket` supplies the shared `SO_REUSEADDR`/`SO_REUSEPORT` behavior required by the same-local-port design; do not bypass it for HTTP, STUN, or listeners.
- HTTP responses and STUN packets are intentionally strict and use absolute response deadlines. Preserve malformed-input rejection and TCP framing.
- Target port `0` means the current public STUN-mapped port. For this dynamic mode, the notification script must succeed before clients are accepted.
- Fixed-target notifications are asynchronous, retry failures, and coalesce pending changes to the newest value. Preserve queue liveness, lock/Condvar ordering, and worker cleanup.
- Notification scripts are executed directly without a command shell and receive exactly five separate arguments: `PUBLIC_IP PUBLIC_PORT LOCAL_PORT TARGET_IP TARGET_PORT`.
- Forwarding is TCP-only and must retain bidirectional half-close/error propagation, idle timeout handling, and the active-client cap.
- Linux/OpenWrt accept errors and process handling are architecture-sensitive; use target-native `libc` constants rather than hardcoded errno numbers.

## Configuration and security

- `--config PATH` conflicts with inline `--http`, `--stun`, and repeatable `--mapping` arguments.
- JSON uses `http`, `stun`, and `mappings`; unknown fields are rejected. Mapping fields are `local_port` (with `local` alias), `target`, and an absolute `script` path. STUN endpoints must support TCP; successful UDP STUN probes are insufficient.
- Script existence/executability is checked when invoked, not while parsing configuration.
- The qBittorrent helper requires `curl`, accepts `QBITTORRENT_URL`, and treats only HTTP 2xx as success. Its documented LAN no-auth assumption is safe only on a trusted LAN.
- Public mappings have no built-in access control. Do not weaken validation, expose credentials, or imply support for NAT types and transports outside the documented scope.
