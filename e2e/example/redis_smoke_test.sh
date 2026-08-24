#!/usr/bin/env bash
# Trivial inner test: proves the runner injected the fixture's endpoint env.
# Nothing actually listens at the address — the scaffold provisions nothing.
set -euo pipefail

: "${REDIS_MAIN_HOST:?REDIS_MAIN_HOST was not injected by the systemtest runner}"
: "${REDIS_MAIN_PORT:?REDIS_MAIN_PORT was not injected by the systemtest runner}"

echo "redis fixture endpoint: ${REDIS_MAIN_HOST}:${REDIS_MAIN_PORT}"
echo "PASS: systemtest plumbing injected the fixture endpoint"
