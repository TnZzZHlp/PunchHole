#!/bin/sh
set -eu

wan_interface=${PUNCHHOLE_WAN_INTERFACE:-pppoe-wan}
lock_file=${PUNCHHOLE_NFT_LOCK:-/var/lock/punchhole-nft.lock}

fail() {
    printf '%s\n' "$1" >&2
    exit 1
}

validate_port() {
    case "$1" in
        ''|*[!0-9]*) fail "invalid port: $1" ;;
    esac
    [ "$1" -ge 1 ] 2>/dev/null && [ "$1" -le 65535 ] 2>/dev/null ||
        fail "invalid port: $1"
}

validate_ipv4() {
    case "$1" in
        ''|*[!0-9.]*) fail "invalid IPv4 address: $1" ;;
    esac
}

case "$wan_interface" in
    ''|*[!A-Za-z0-9_.:-]*) fail "invalid WAN interface: $wan_interface" ;;
esac

create_table() {
    nft -f - <<EOF
table ip punchhole {
    set ports {
        type inet_service
    }

    map forwards {
        type inet_service : ipv4_addr . inet_service
    }

    chain prerouting {
        type nat hook prerouting priority -101; policy accept;
        iifname "$wan_interface" ip protocol tcp ip dscp cs1 ip dscp set cs0
        iifname "$wan_interface" ip protocol tcp tcp dport @ports ip dscp set cs1 counter dnat ip to tcp dport map @forwards
    }
}
EOF
}

ensure_table() {
    nft list table ip punchhole >/dev/null 2>&1 || create_table
}

exec 9>"$lock_file"
flock -x 9

if [ "$#" -eq 1 ]; then
    case "$1" in
        --init)
            nft delete table ip punchhole 2>/dev/null || true
            create_table
            exit 0
            ;;
        --clear)
            ensure_table
            nft flush set ip punchhole ports
            nft flush map ip punchhole forwards
            exit 0
            ;;
    esac
fi

[ "$#" -eq 3 ] || fail \
    "usage: $0 PUBLIC_IP PUBLIC_PORT LOCAL_PORT"

public_ip=$1
public_port=$2
local_port=$3
target_ip=${PUNCHHOLE_TARGET_IP:-192.168.2.10}
target_port=${PUNCHHOLE_TARGET_PORT:-$public_port}

validate_ipv4 "$public_ip"
validate_port "$public_port"
validate_port "$local_port"
validate_ipv4 "$target_ip"
validate_port "$target_port"

ensure_table
nft "delete element ip punchhole forwards { $local_port }" 2>/dev/null || true
nft "add element ip punchhole forwards { $local_port : $target_ip . $target_port }"
nft "add element ip punchhole ports { $local_port }" 2>/dev/null ||
    nft "get element ip punchhole ports { $local_port }" >/dev/null
