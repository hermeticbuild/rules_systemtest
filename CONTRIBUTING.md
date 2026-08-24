# How to Contribute

## Using devcontainers

If you are using [devcontainers](https://code.visualstudio.com/docs/devcontainers/containers)
and/or [codespaces](https://github.com/features/codespaces) then you can start
contributing immediately and skip the next step.

## Formatting

Starlark files should be formatted by buildifier.
We suggest using a pre-commit hook to automate this.
First [install pre-commit](https://pre-commit.com/#installation),
then run

```shell
pre-commit install
pre-commit install --hook-type commit-msg
```

Otherwise later tooling on CI will yell at you about formatting/linting violations.

## Commit messages

Commits and pull request titles must follow
[Conventional Commits](https://www.conventionalcommits.org/). CI enforces this on the PR title,
and the `commitizen` pre-commit hook enforces it on commit messages locally. Release automation
is not set up yet, but it will derive versions from this history, so it is worth getting right
from the start.

## Updating BUILD files

Some targets are generated from sources.
Currently this is just the `bzl_library` targets.
Run `bazel run //:gazelle` to keep them up-to-date.

## Updating .bazelrc presets

`tools/preset.bazelrc` is generated. Run `bazel run //tools:preset.update` after changing the
`bazelrc_preset` target.

## Using this as a development dependency of other rules

You'll commonly find that you develop in another workspace, such as
some other ruleset that depends on rules_systemtest, or in a nested
workspace under tests/e2e.

To always tell Bazel to use this directory rather than some release
artifact or a version fetched from the internet, include this in the
MODULE.bazel file.

```starlark
local_path_override(
    module_name = "rules_systemtest",
    path = "path/to/rules_systemtest",
)
```

This means that any usage of `@rules_systemtest` on your system will point to this folder.

## Releasing

Not set up yet. Releases and Bazel Central Registry publishing will be added later.
