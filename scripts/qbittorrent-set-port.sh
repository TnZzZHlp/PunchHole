#!/bin/sh
set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 PUBLIC_IP PUBLIC_PORT LOCAL_PORT TARGET_IP TARGET_PORT" >&2
    exit 2
fi

public_port=$2
case "$public_port" in
    ''|*[!0-9]*)
        echo "invalid public port: $public_port" >&2
        exit 2
        ;;
esac
if [ "$public_port" -lt 1 ] || [ "$public_port" -gt 65535 ]; then
    echo "public port must be between 1 and 65535" >&2
    exit 2
fi

base_url=${QBITTORRENT_URL:-http://192.168.2.10:8080}
base_url=${base_url%/}
json=$(printf '{"listen_port":%s}' "$public_port")

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
