#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 PUBLIC_IP PUBLIC_PORT LOCAL_PORT" >&2
    exit 2
fi

validate_port() {
    case "$1" in
        ''|*[!0-9]*)
            echo "invalid port: $1" >&2
            exit 2
            ;;
    esac
    if [ "$1" -lt 1 ] || [ "$1" -gt 65535 ]; then
        echo "port must be between 1 and 65535: $1" >&2
        exit 2
    fi
}

public_port=$2
listen_port=${QBITTORRENT_LISTEN_PORT:-$public_port}
announce_port=${QBITTORRENT_ANNOUNCE_PORT:-}
validate_port "$public_port"
validate_port "$listen_port"

base_url=${QBITTORRENT_URL:-http://192.168.2.10:8080}
base_url=${base_url%/}
if [ -n "$announce_port" ]; then
    validate_port "$announce_port"
    json=$(printf '{"listen_port":%s,"announce_port":%s}' "$listen_port" "$announce_port")
else
    json=$(printf '{"listen_port":%s}' "$listen_port")
fi

status=$(curl --fail --silent --show-error --max-time 10 \
    -X POST \
    -H "Referer: ${base_url}/" \
    --data-urlencode "json=$json" \
    --output /dev/null \
    --write-out '%{http_code}' \
    "${base_url}/api/v2/app/setPreferences")
case "$status" in
    2??) ;;
    *)
        echo "qBittorrent API returned HTTP status: $status" >&2
        exit 1
        ;;
esac
