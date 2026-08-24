#!/bin/sh
# Reformat and regenerate everything checked in.
#
# CI asserts this script produces no changes, so anything it rewrites must be
# committed. See .github/workflows/ci.yaml.
set -eu

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${REPO_ROOT}/src"

GO_PROTO_PKG="proto/systemtestpb"   # relative to ${SRC}
RUST_GENERATED="proto/systemtestrs" # relative to ${SRC}

if ! command -v bazel >/dev/null 2>&1; then
    echo "ERROR: bazel is required" >&2
    exit 1
fi

# MODULE.bazel.lock refresh. `bazel mod deps` would be the obvious command, but
# we can't due to https://github.com/protocolbuffers/protobuf/issues/28224
echo "==> (main) bzlmod lockfile update"
bazel build --nobuild --lockfile_mode=update //...

cd "${SRC}"

echo "==> (src) bzlmod lockfile update"
bazel build --nobuild --lockfile_mode=update //...

echo "==> (src) bazel run //tools:write_generated_srcs"
bazel run //tools:write_generated_srcs

echo "==> (src) go mod tidy"
bazel run -- @rules_go//go mod tidy

echo "==> gofmt"
GOROOT="$(bazel run -- @rules_go//go env GOROOT 2>/dev/null)"
# Skip the generated package: protoc-gen-go already emits gofmt-clean output.
find "${REPO_ROOT}" -name '*.go' -not -path "${SRC}/${GO_PROTO_PKG}/*" \
    -exec "${GOROOT}/bin/gofmt" -s -w {} +

# rustfmt. Skip the generated crate: prost/tonic emit their own formatting.
echo "==> rustfmt"
find "${SRC}" -name '*.rs' -not -path "${SRC}/${RUST_GENERATED}/*" -print0 |
    xargs -0 bazel run -- @rules_rust//tools/upstream_wrapper:rustfmt --edition 2024

# buildifier the whole repo
echo "==> buildifier"
cd "${REPO_ROOT}"
bazel run @buildifier_prebuilt//:buildifier -- -r "${REPO_ROOT}"

echo "==> done"
