# PunchHole

PunchHole keeps direct IPv4 TCP mappings open from a Linux or OpenWrt device
behind a full-cone (NAT1) router. One process can maintain multiple independent
mappings; each mapping has its own source/listen port, target, and notification
script.

## Build and checks

```sh
cargo build --release --locked
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

## Logging

PunchHole writes structured tracing output at INFO level by default. Set
`RUST_LOG=debug` for more detailed output, for example:

```sh
RUST_LOG=debug PunchHole --config config.json
```

## Configuration and CLI

The process requires one existing HTTP service and one existing STUN service.
The STUN service must accept STUN over TCP and return an IPv4
`XOR-MAPPED-ADDRESS` in a Binding Success Response; a UDP-only STUN service
will not work. The HTTP service must return a valid persistent response:
HTTP/1.0 with `Connection: keep-alive`, or HTTP/1.1 without
`Connection: close`; PunchHole sends periodic `HEAD /` requests.

Configuration can be supplied as a strict JSON file:

```json
{
  "http": "198.51.100.10:80",
  "stun": "198.51.100.20:3478",
  "mappings": [
    {
      "local_port": 10001,
      "target": "192.168.2.10:0",
      "script": "/absolute/path/PunchHole/scripts/qbittorrent-set-port.sh"
    }
  ]
}
```

Start it with:

```sh
PunchHole --config config.json
```

The JSON schema has exactly the `http`, `stun`, and `mappings` fields shown
above. Each mapping has a numeric `local_port`, a `target`, and a `script`;
`local` is accepted as an alias for `local_port`. JSON fields and value types
are strict, unknown fields are rejected, and script paths must be absolute. A
target port of `0` is dynamic: after STUN reports the public port, the mapping
forwards to `IPv4_ADDRESS:<public-port>`. Fixed numeric target ports continue
to work normally.

The preserved direct CLI form uses repeatable `--mapping` options. Its fields
are:

```text
local=PRIVATE_PORT,target=HOST_OR_IPV4:PORT,script=ABSOLUTE_PATH
```

Generic example:

```sh
./target/release/PunchHole \
  --http 198.51.100.10:80 \
  --stun 198.51.100.20:3478 \
  --mapping 'local=10001,target=192.168.1.20:22,script=/opt/app1.sh' \
  --mapping 'local=10002,target=127.0.0.1:8080,script=/opt/app2.sh'
```

The addresses above use TEST-NET documentation ranges and the script paths
are placeholders; replace all of them with real values. HTTP, STUN, and target
endpoints may use an IPv4 literal or DNS hostname followed by a port. Hostnames
are resolved once when configuration is loaded, and the first IPv4 result is
used; restart PunchHole to pick up DNS changes. IPv6-only names are rejected.
Local ports must be unique and non-zero. The configured local port is used for
the persistent HTTP connection, the TCP STUN Binding request, and the local
TCP listener. Linux/OpenWrt must support `SO_REUSEPORT` for this same-port
setup.

When a public endpoint is first established or changes after a retry, PunchHole
runs the mapping script without a shell. Script paths must be absolute; missing
or non-executable scripts are reported when invoked. Arguments are passed in
this order:

```text
script PUBLIC_IP PUBLIC_PORT LOCAL_PORT TARGET_IP TARGET_PORT
```

For a dynamic target, `TARGET_PORT` is the resolved public port. Fixed-target
notifications run asynchronously and coalesce pending updates. Dynamic-target
notifications finish before the listener accepts clients; a failed or timed-out
dynamic notification causes that mapping to retry.

## qBittorrent TCP mapping

`192.168.2.10:8080` is the qBittorrent WebUI/API endpoint, not the BitTorrent
peer port. The included `scripts/qbittorrent-set-port.sh` calls
`/api/v2/app/setPreferences` and changes qBittorrent's `listen_port` to the
current public mapped port. It defaults to the user's LAN endpoint and can be
overridden with `QBITTORRENT_URL`.

Example:

```sh
./target/release/PunchHole \
  --http 198.51.100.10:80 \
  --stun 198.51.100.20:3478 \
  --mapping 'local=10001,target=192.168.2.10:0,script=/absolute/path/PunchHole/scripts/qbittorrent-set-port.sh'
```

This assumes the qBittorrent WebAPI allows unauthenticated access from the LAN
and that `curl` is installed on the device. The dynamic target is applied only
after the script completes, so incoming TCP traffic is forwarded to the port
qBittorrent just selected. This implementation covers BitTorrent TCP only; UDP
and uTP traffic are not forwarded.

## Scope and limitations

This is direct-only hole punching. It relies on IPv4 TCP behavior of a
full-cone/NAT1 device and does not promise connectivity through ordinary,
restricted, symmetric, or carrier-grade NAT. It does not implement TURN,
relays, an HTTP server, or a STUN server. Clients must connect to the public
IPv4:port reported by STUN.

**Security warning:** direct mappings have no built-in client authentication or
ACL. Any client that can reach a public mapping and is allowed by the NAT is
forwarded to the configured target. The qB LAN no-auth setting is only safe on a
trusted LAN; use an authenticated target service and appropriate host/firewall
controls for any exposed service.
