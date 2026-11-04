#!/usr/bin/env bash
# Trivial inner test: proves the runner injected the fixture's endpoint env.
# Nothing actually listens at the address — the scaffold provisions nothing.
set -euo pipefail

: "${REDIS_MAIN_HOST:?REDIS_MAIN_HOST was not injected by the systemtest runner}"
: "${REDIS_MAIN_PORT:?REDIS_MAIN_PORT was not injected by the systemtest runner}"

# The "extra" endpoint comes from the `config` file input (JSON merged into the
# ports by the dummy plugin after the runner inlined the file's contents).
: "${REDIS_EXTRA_PORT:?REDIS_EXTRA_PORT (from the config file input) was not injected}"
if [ "${REDIS_EXTRA_PORT}" != "7000" ]; then
    echo "FAIL: expected REDIS_EXTRA_PORT=7000 from config file, got ${REDIS_EXTRA_PORT}" >&2
    exit 1
fi

echo "redis fixture endpoint: ${REDIS_MAIN_HOST}:${REDIS_MAIN_PORT} (extra port ${REDIS_EXTRA_PORT})"
echo "PASS: systemtest plumbing injected the fixture endpoint and inlined the config file"
