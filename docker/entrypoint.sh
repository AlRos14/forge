#!/bin/sh
set -eu

DATA_DIR="${FORGE_DATA_DIR:-/data}"
APP_USER="${FORGE_CONTAINER_USER:-forge}"
APP_GROUP="${FORGE_CONTAINER_GROUP:-forge}"

if [ "$#" -eq 0 ]; then
    set -- forge
elif [ "${1#-}" != "$1" ]; then
    # Allow `docker run IMAGE --help` while keeping an explicit command like
    # `docker run IMAGE forge-ctl ...` unchanged.
    set -- forge "$@"
fi

if [ "$(id -u)" -ne 0 ]; then
    # An explicit --user already chose the runtime identity. Do not attempt
    # privileged ownership changes.
    exec "$@"
fi

if ! id "$APP_USER" >/dev/null 2>&1; then
    echo "forge-entrypoint: runtime user '$APP_USER' does not exist" >&2
    exit 1
fi

mkdir -p "$DATA_DIR"

root_owned="$(find "$DATA_DIR" -xdev -user root -print -quit 2>/dev/null || true)"
if [ -n "$root_owned" ]; then
    if [ "${FORGE_MIGRATE_ROOT_OWNED_DATA:-0}" = "1" ]; then
        echo "forge-entrypoint: migrating root-owned data under $DATA_DIR to $APP_USER (explicit opt-in)" >&2
        find "$DATA_DIR" -xdev \( -user root -o -group root \) \
            -exec chown "$APP_USER:$APP_GROUP" {} +
    elif [ -z "$(find "$DATA_DIR" -mindepth 1 -print -quit 2>/dev/null | head -n 1)" ]; then
        # A fresh Docker volume is mounted root-owned and empty. Claim only the
        # directory itself; there is no user data to rewrite.
        chown "$APP_USER:$APP_GROUP" "$DATA_DIR"
    else
        echo "forge-entrypoint: $DATA_DIR contains root-owned data from a previous image." >&2
        echo "forge-entrypoint: run once with FORGE_MIGRATE_ROOT_OWNED_DATA=1 to migrate ownership," >&2
        echo "forge-entrypoint: or continue using the previous image. No files were changed." >&2
        exit 1
    fi
fi

if [ -n "${FORGE_WORKSPACE_ROOT:-}" ] && [ -d "$FORGE_WORKSPACE_ROOT" ]; then
    if [ -n "$(find "$FORGE_WORKSPACE_ROOT" -xdev -user root -print -quit 2>/dev/null || true)" ]; then
        echo "forge-entrypoint: workspace root $FORGE_WORKSPACE_ROOT contains root-owned entries." >&2
        echo "forge-entrypoint: fix the mount ownership explicitly; no recursive chown was attempted." >&2
        exit 1
    fi
fi

exec gosu "$APP_USER:$APP_GROUP" "$@"
