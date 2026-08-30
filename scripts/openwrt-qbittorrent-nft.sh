#!/bin/sh
set -eu

[ "$#" -eq 3 ] || {
    printf '%s\n' "usage: $0 PUBLIC_IP PUBLIC_PORT LOCAL_PORT" >&2
    exit 2
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
QBITTORRENT_LISTEN_PORT=$2 QBITTORRENT_ANNOUNCE_PORT=$2 \
    "$script_dir/qbittorrent-set-port.sh" "$@"
exec "$script_dir/openwrt-nft-forward.sh" "$@"
