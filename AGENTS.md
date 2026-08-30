# Project Guidance

## Overview

PunchHole is a Rust 2024 command-line service for maintaining multiple direct IPv4 TCP mappings behind a full-cone/NAT1 router. It reuses the same local port for an HTTP keepalive connection and a TCP STUN request, then delegates data-plane forwarding to notification scripts. It is direct-only: there is no built-in payload proxy, TURN, relay, UDP/uTP forwarding, CGNAT support, authentication, or ACL.

## Repository map

- `src/main.rs`: minimal binary entry point.
- `src/lib.rs`: module wiring, tracing initialization, and public test-facing re-exports.
- `src/cli.rs`: `clap` command-line definition.
- `src/config.rs`: strict JSON-only configuration parsing and mapping validation.
- `src/net.rs`: reusable same-port bound sockets and outbound TCP setup.
- `src/http.rs`: persistent HTTP `HEAD` setup, strict response parsing, and keepalive loop.
- `src/stun.rs`: TCP STUN request framing and strict IPv4 `XOR-MAPPED-ADDRESS` parsing.
- `src/mapping.rs`: mapping supervision, notification, and steady HTTP keepalive lifecycle.
- `src/notify.rs`: synchronous script execution, timeout, and Linux process-group cleanup.
- `tests/`: integration tests grouped by module. Keep tests here; do not add inline test modules under `src/`.
- `example.config.json`: strict JSON example using documentation-only endpoints and an absolute placeholder script path.
- `scripts/qbittorrent-set-port.sh`: POSIX shell helper for qBittorrent WebAPI port updates.
- `scripts/openwrt-nft-forward.sh`: privileged nftables DNAT setup and dynamic map updates.
- `scripts/openwrt-qbittorrent-nft.sh`: combined qBittorrent update and OpenWrt DNAT notification script.

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

Inspect any automatic fixes for networking, ownership, timeout, and lock-scope changes, then rerun formatting, tests, and strict Clippy. Do not hide failures with broad lint allowances. If shell helpers change, also run:

```sh
sh -n scripts/*.sh
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

- HTTP and STUN endpoints accept IPv4 literals or DNS hostnames with a port. Hostnames resolve once at configuration load to the first IPv4 result; runtime networking remains IPv4-only.
- Each mapping obtains an OS-selected local port when it first establishes HTTP. That port remains fixed through retries for the process lifetime and is reused for HTTP and TCP STUN.
- `net::new_bound_socket` supplies the shared `SO_REUSEADDR`/`SO_REUSEPORT` behavior required by the same-local-port design; do not bypass it for HTTP or STUN.
- HTTP responses and STUN packets are intentionally strict and use absolute response deadlines. Preserve malformed-input rejection and TCP framing.
- Notification scripts must succeed before steady keepalive. They execute directly without a shell and receive exactly three separate arguments: `PUBLIC_IP PUBLIC_PORT LOCAL_PORT`.
- Rust code must not accept client connections or proxy payload traffic. Data-plane forwarding belongs in the configured script; the included OpenWrt path uses kernel nftables DNAT.
- Linux/OpenWrt process handling is architecture-sensitive; use target-native `libc` constants rather than hardcoded errno numbers.

## Configuration and security

- `--config PATH` is the only configuration form; inline HTTP, STUN, and mapping arguments are unsupported.
- JSON uses `http`, `stun`, and `mappings`; unknown fields are rejected. Each mapping has exactly one absolute `script` path. STUN endpoints must support TCP; successful UDP STUN probes are insufficient.
- Script existence/executability is checked when invoked, not while parsing configuration.
- The qBittorrent helper requires `curl`, accepts `QBITTORRENT_URL`, optional `QBITTORRENT_LISTEN_PORT`, and optional `QBITTORRENT_ANNOUNCE_PORT`, and treats only HTTP 2xx as success. Its documented LAN no-auth assumption is safe only on a trusted LAN.
- The OpenWrt DNAT helper requires `nft`, `flock`, and root or `CAP_NET_ADMIN`. Its target is script-controlled through `PUNCHHOLE_TARGET_IP` and `PUNCHHOLE_TARGET_PORT`; privileged service configuration and scripts must remain root-owned.
- Public mappings have no built-in access control. Do not weaken validation, expose credentials, or imply support for NAT types and transports outside the documented scope.
