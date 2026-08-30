# PunchHole

PunchHole maintains direct IPv4 TCP mappings behind a full-cone/NAT1 router.
It reuses one OS-selected local port for an HTTP keepalive connection and TCP
STUN, then invokes a script to install the data-plane forwarding. PunchHole
does not proxy payload traffic.

## Build and checks

```sh
cargo build --release --locked
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

## Configuration

PunchHole accepts only a strict JSON configuration file:

```json
{
  "http": "198.51.100.10:80",
  "stun": "198.51.100.20:3478",
  "mappings": [
    {
      "script": "/absolute/path/PunchHole/scripts/openwrt-qbittorrent-nft.sh"
    }
  ]
}
```

Start it with:

```sh
PunchHole --config config.json
```

The top-level fields are exactly `http`, `stun`, and `mappings`. Each mapping
contains only an absolute `script` path. Unknown fields are rejected.

HTTP and STUN endpoints may use an IPv4 literal or DNS hostname with a port.
Hostnames resolve once at configuration load to the first IPv4 result. The
STUN endpoint must support STUN over TCP and return an IPv4
`XOR-MAPPED-ADDRESS`; UDP-only STUN is insufficient. The HTTP endpoint must
return a persistent HTTP/1.0 or HTTP/1.1 response to periodic `HEAD /` requests.

Each mapping obtains a random local port from the operating system when its
first HTTP connection succeeds. That port remains fixed through retries until
PunchHole restarts. Linux/OpenWrt must support `SO_REUSEPORT` so HTTP and STUN
can share it.

## Notification script

When STUN first reports a public endpoint or that endpoint changes, PunchHole
executes the mapping script directly without a shell. It passes exactly three
separate arguments:

```text
script PUBLIC_IP PUBLIC_PORT LOCAL_PORT
```

The script must establish forwarding before exiting. A missing, failed, or
timed-out script causes the mapping to retry. Target selection belongs entirely
to the script and is not part of PunchHole configuration.

## OpenWrt qBittorrent mapping

`scripts/openwrt-qbittorrent-nft.sh` combines two helpers:

1. `qbittorrent-set-port.sh` sets qBittorrent `listen_port` and `announce_port`
   to `PUBLIC_PORT`.
2. `openwrt-nft-forward.sh` maps random `LOCAL_PORT` to the chosen target with
   kernel nftables DNAT.

The helpers use these environment variables:

```text
QBITTORRENT_URL          default: http://192.168.2.10:8080
PUNCHHOLE_TARGET_IP      default: 192.168.2.10
PUNCHHOLE_TARGET_PORT    default: PUBLIC_PORT
PUNCHHOLE_WAN_INTERFACE default: pppoe-wan
```

Use a small wrapper per qBittorrent instance to select its WebAPI and target.
For example:

```sh
#!/bin/sh
export QBITTORRENT_URL=http://192.168.2.10:8080
export PUNCHHOLE_TARGET_IP=192.168.2.10
exec /usr/local/lib/punchhole/openwrt-qbittorrent-nft.sh "$@"
```

Run the nft helper with `--init` before starting PunchHole to remove stale
mappings:

```sh
/usr/local/lib/punchhole/openwrt-nft-forward.sh --init
```

The OpenWrt helper requires `nft`, `flock`, and root or `CAP_NET_ADMIN`. It
clears incoming forged CS1 marks and marks only PunchHole DNAT flows as CS1. If
the target host uses a default-deny input firewall, accept marked TCP packets
on its LAN interface before the final drop rule, for example:

```nft
 iifname "eno1" ip dscp cs1 ip protocol tcp accept
```

The qBittorrent helper requires `curl` and assumes its LAN WebAPI permits the
request. The included path forwards TCP only; UDP and uTP are outside scope.

## Logging

PunchHole writes structured tracing output at INFO level by default:

```sh
RUST_LOG=debug PunchHole --config config.json
```

A `mapping ready` record contains the randomly selected local port and public
endpoint.

## Scope and security

This is direct-only hole punching for full-cone/NAT1 IPv4 behavior. It does not
implement TURN, relay, UDP forwarding, authentication, ACLs, or CGNAT support.
Upstream carrier NAT may still prevent arbitrary-source connectivity.

Script-installed mappings expose their target without application-level access
control. Keep configuration and privileged scripts root-owned, protect the
qBittorrent WebAPI on a trusted LAN, and apply firewall policy appropriate to
the exposed service.
