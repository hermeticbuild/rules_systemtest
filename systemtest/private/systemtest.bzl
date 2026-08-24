"""The `systemtest` test rule: the `bazel test` entry point.

It ties a set of fixtures to an inner test binary. At analysis time it collects
each fixture's serialized description + runfiles and the inner test, resolves
the runner from the runner toolchain, and emits a launcher that invokes the
runner with a single run_config.json (design "The systemtest test rule").
"""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load(":config.bzl", "COORDINATOR_MODE_LOCAL")
load(":fixture.bzl", "SystemtestFixtureInfo")

_RUNNER_TOOLCHAIN_TYPE = "//systemtest/toolchains:runner_toolchain_type"
_COORDINATOR_TOOLCHAIN_TYPE = "//systemtest/toolchains:coordinator_toolchain_type"

# Both toolchain types are declared optional so a missing one produces this
# instead of Bazel's bare "no matching toolchains found for types ...".
_MISSING_TOOLCHAIN = """\
no {what} is registered for {type}.

rules_systemtest ships precompiled binaries, but none matched (no release for
this platform, or an unreleased version of the ruleset). To build them from
source, add to your MODULE.bazel:

    bazel_dep(name = "rules_systemtest_src", version = "<rules_systemtest version>")
    register_toolchains("@rules_systemtest_src//toolchains:all")
"""

def _rlocation(ctx, file):
    if file.short_path.startswith("../"):
        return file.short_path[len("../"):]
    return ctx.workspace_name + "/" + file.short_path

def _resolve(ctx, toolchain_type, field, what):
    toolchain = ctx.toolchains[toolchain_type]
    if toolchain == None:
        fail(_MISSING_TOOLCHAIN.format(what = what, type = toolchain_type))
    return getattr(toolchain, field)

def _systemtest_impl(ctx):
    mode = ctx.attr._coordinator_mode[BuildSettingInfo].value
    if mode != COORDINATOR_MODE_LOCAL:
        fail("scaffold only supports coordinator_mode=local, got %r" % mode)

    runner = _resolve(ctx, _RUNNER_TOOLCHAIN_TYPE, "runner", "systemtest runner")
    coordinator_info = _resolve(
        ctx,
        _COORDINATOR_TOOLCHAIN_TYPE,
        "coordinator",
        "systemtest coordinator",
    )
    coordinator = coordinator_info.binary
    inner_test = ctx.executable.test

    fixtures_config = []
    all_runfiles = ctx.runfiles()
    description_files = []
    for target in ctx.attr.fixtures:
        info = target[SystemtestFixtureInfo]
        description_files.append(info.description_file)
        fixtures_config.append({
            "name": info.fixture_name,
            "description_path": _rlocation(ctx, info.description_file),
        })
        all_runfiles = all_runfiles.merge(info.runfiles)

    run_config = {
        "systemtest_label": str(ctx.label),
        "mode": mode,
        "coordinator_bin": _rlocation(ctx, coordinator),
        "lease_ttl_seconds": 60,
        "startup_timeout_seconds": 300,
        "fixtures": fixtures_config,
        "test": {
            "binary_path": _rlocation(ctx, inner_test),
            "args": ctx.attr.test_args,
            "env": ctx.attr.test_env,
        },
    }
    config_file = ctx.actions.declare_file(ctx.label.name + ".run_config.json")
    ctx.actions.write(config_file, json.encode_indent(run_config, indent = "  "))

    launcher = ctx.actions.declare_file(ctx.label.name + ".launcher.sh")
    ctx.actions.expand_template(
        template = ctx.file._launcher_tpl,
        output = launcher,
        substitutions = {
            "%RUNNER%": _rlocation(ctx, runner.binary),
            "%CONFIG%": _rlocation(ctx, config_file),
        },
        is_executable = True,
    )

    # One merged runfiles tree so the runner's rlocation of coordinator/plugin/
    # inner-test all resolve from the same place.
    all_runfiles = all_runfiles.merge_all([
        ctx.runfiles(files = [config_file, coordinator, inner_test] + description_files),
        runner.runfiles,
        coordinator_info.runfiles,
        ctx.attr.test[DefaultInfo].default_runfiles,
        ctx.attr._bash_runfiles[DefaultInfo].default_runfiles,
    ])

    return [
        DefaultInfo(executable = launcher, runfiles = all_runfiles),
        RunEnvironmentInfo(inherited_environment = ["XML_OUTPUT_FILE"]),
    ]

_systemtest_test = rule(
    implementation = _systemtest_impl,
    test = True,
    attrs = {
        "test": attr.label(
            mandatory = True,
            executable = True,
            cfg = "target",
            doc = "The inner *_test binary to run against the fixtures.",
        ),
        "fixtures": attr.label_list(
            providers = [SystemtestFixtureInfo],
            doc = "Fixtures to lease for the duration of the test.",
        ),
        "test_args": attr.string_list(doc = "Extra args passed to the inner test."),
        "test_env": attr.string_dict(doc = "Extra env for the inner test."),
        "_coordinator_mode": attr.label(default = Label("//:coordinator_mode")),
        "_launcher_tpl": attr.label(
            default = Label("//systemtest/private:launcher.sh.tpl"),
            allow_single_file = True,
        ),
        "_bash_runfiles": attr.label(
            default = Label("@bazel_tools//tools/bash/runfiles"),
        ),
    },
    toolchains = [
        config_common.toolchain_type(_RUNNER_TOOLCHAIN_TYPE, mandatory = False),
        config_common.toolchain_type(_COORDINATOR_TOOLCHAIN_TYPE, mandatory = False),
    ],
    doc = "Runs an inner test with a set of leased fixtures.",
)

def systemtest(name, **kwargs):
    """Runs an inner test with a set of leased fixtures.

    Thin wrapper over the underlying rule: Bazel requires a test rule's class
    name to end in `_test`, but the public API is `systemtest`, so the rule is
    `_systemtest_test` and this macro exposes it under the intended name.

    Args:
      name: target name.
      **kwargs: forwarded to the rule (test, fixtures, test_args, test_env, and
                common attrs like visibility/tags/size).
    """
    _systemtest_test(name = name, **kwargs)
