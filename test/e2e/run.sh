#!/bin/sh
# The browser test of LispM Remote's page, on Linux, as root:
#
#   sudo test/e2e/run.sh
#
# It needs the module (`make module`), the page (`make page`) and this
# directory's packages with Playwright's Chromium (`npm ci && npx playwright
# install chromium`).  The browser runs in a network namespace of its own,
# joined to this computer by a pair of virtual links, so that `ip link`
# really cuts its network; LispM's Emacs (lispm.el) runs outside it, its
# relay node answering as the seed.  Both run as the user who called sudo.
#
# Copyright (C) 2026  Ryan.K
# SPDX-License-Identifier: AGPL-3.0-or-later

set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
user=${SUDO_USER:-$(id -un)}
ns=lispm-e2e
host_if=lispm-e2e-h
ns_if=lispm-e2e-n
host_ip=10.77.0.1
ns_ip=10.77.0.2
port=50077
dir=$(mktemp -d)
chown "$user" "$dir"
emacs_pid=
server_pid=

cleanup() {
  status=$?
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  [ -n "$emacs_pid" ] && kill "$emacs_pid" 2>/dev/null || true
  ip netns del "$ns" 2>/dev/null || true
  ip link del "$host_if" 2>/dev/null || true
  if [ "$status" -ne 0 ]; then
    echo "--- LispM's Emacs:"
    cat "$dir/emacs.log" 2>/dev/null || true
    if [ -n "${E2E_ARTIFACTS:-}" ]; then
      mkdir -p "$E2E_ARTIFACTS"
      cp "$dir"/*.log "$dir"/*.png "$E2E_ARTIFACTS" 2>/dev/null || true
    fi
  fi
  rm -rf "$dir"
  exit "$status"
}
trap cleanup EXIT INT TERM

# Commands run as the user through setpriv, which execs them, as `ip netns
# exec` does: so $! of one run in the background is the command itself,
# which cleanup stops.
uid=$(id -u "$user")
gid=$(id -g "$user")
home=$(getent passwd "$user" | cut -d: -f6)

# The browser's own network, linked to this computer's.
ip netns del "$ns" 2>/dev/null || true
ip link del "$host_if" 2>/dev/null || true
ip netns add "$ns"
ip link add "$host_if" type veth peer name "$ns_if"
ip link set "$ns_if" netns "$ns"
ip addr add "$host_ip/24" dev "$host_if"
ip link set "$host_if" up
ip netns exec "$ns" ip addr add "$ns_ip/24" dev "$ns_if"
ip netns exec "$ns" ip link set "$ns_if" up
ip netns exec "$ns" ip link set lo up

# LispM, outside it.
cd "$root"
setpriv --reuid="$uid" --regid="$gid" --init-groups \
  env PATH="$PATH" HOME="$home" E2E_DIR="$dir" E2E_HOST="$host_ip" E2E_PORT="$port" \
  emacs -Q --batch -L lisp -L test -l test/e2e/lispm.el >"$dir/emacs.log" 2>&1 &
emacs_pid=$!
i=0
until [ -s "$dir/ready" ]; do
  i=$((i + 1))
  if [ "$i" -gt 600 ] || ! kill -0 "$emacs_pid" 2>/dev/null; then
    echo "LispM's Emacs did not start"
    exit 1
  fi
  sleep 0.2
done

# The page and the browser, inside it.
ip netns exec "$ns" setpriv --reuid="$uid" --regid="$gid" --init-groups \
  python3 -m http.server --bind 127.0.0.1 --directory "$root/web/dist" 8000 \
  >"$dir/server.log" 2>&1 &
server_pid=$!
ip netns exec "$ns" setpriv --reuid="$uid" --regid="$gid" --init-groups \
  env PATH="$PATH" HOME="$home" \
  E2E_DIR="$dir" E2E_PAGE="http://127.0.0.1:8000/" E2E_SEED="http://seed.lispm.test:$port" \
  E2E_SEED_IP="$host_ip" E2E_IFACE="$ns_if" \
  node "$root/test/e2e/e2e.mjs"
