# How to Contribute

## Formatting

Run './tools/format.sh' to make sure code is formatted.

## Commit messages

Commits and pull request titles must follow
[Conventional Commits](https://www.conventionalcommits.org/). CI enforces this.

## Updating BUILD files

Some targets are generated from sources.
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
