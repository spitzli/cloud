#!/bin/sh
# Railpack clears proxy variables when it starts Mise. Read only the validated operator handoff.
set -eu
if [ -s /workspace/runtime/package-proxy-url ]; then
    IFS= read -r package_proxy </workspace/runtime/package-proxy-url
    case "$package_proxy" in
        http://10.83.0.1:3128) ;;
        *) echo invalid_package_proxy >&2; exit 1 ;;
    esac
    export HTTP_PROXY="$package_proxy" HTTPS_PROXY="$package_proxy" ALL_PROXY="$package_proxy"
    export http_proxy="$package_proxy" https_proxy="$package_proxy" all_proxy="$package_proxy"
    export NO_PROXY= no_proxy=
fi
exec /usr/local/libexec/mise-2026.9.15 "$@"
