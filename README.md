# Bazel rules for system tests

`rules_systemtest` runs tests against external services — a database, a message broker, your
own service under test — that are provisioned out-of-process, leased for the duration of a
test, and shared and reused across tests until they go idle.

See [DESIGN.md](./DESIGN.md) for the design, and for guidance on whether this ruleset or the
lighter [`rules_itest`](https://github.com/hermeticbuild/rules_itest) fits your tests.

## Status

Early development. There are no releases yet and the public API is not implemented.

## Installation

There is no release to point at yet. To develop against the ruleset, point at a commit with an
`archive_override` in MODULE.bazel.

For example to use commit `abc123`:

```starlark
bazel_dep(name = "rules_systemtest", version = "0.0.0")

archive_override(
    module_name = "rules_systemtest",
    url = "https://github.com/hermeticbuild/rules_systemtest/archive/abc123.tar.gz",
    strip_prefix = "rules_systemtest-abc123",
    # The easiest way to set this is to comment out this line, then Bazel will print
    # a message with the correct value. Note that GitHub source archives don't have a strong
    # guarantee on the sha256 stability, see <https://github.blog/2023-02-21-update-on-the-future-stability-of-source-code-archives-and-hashes/>
    integrity = "...",
)
```

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).
