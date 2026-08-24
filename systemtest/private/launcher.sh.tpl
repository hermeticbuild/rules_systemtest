#!/usr/bin/env bash
# Launcher emitted by the systemtest rule. Resolves the runner + run-config from
# runfiles and hands off to the runner. All other config lives in the JSON.
set -euo pipefail

# --- begin runfiles.bash initialization v3 ---
# Copy-pasted standard snippet (see rules_bash docs).
set -uo pipefail; set +e; f=bazel_tools/tools/bash/runfiles/runfiles.bash
source "${RUNFILES_DIR:-/dev/null}/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "${RUNFILES_MANIFEST_FILE:-/dev/null}" | cut -f2- -d' ')" 2>/dev/null || \
  source "$0.runfiles/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.exe.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  { echo>&2 "ERROR: cannot find $f"; exit 1; }; f=; set -e
# --- end runfiles.bash initialization v3 ---

RUNNER="$(rlocation "%RUNNER%")"
CONFIG="$(rlocation "%CONFIG%")"

exec "$RUNNER" run --run-config "$CONFIG"
