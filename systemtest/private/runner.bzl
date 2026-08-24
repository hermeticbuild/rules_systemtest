"""The runner toolchain: systemtest_runner + systemtest_runner_toolchain.

The runner is a normal binary. Users can supply their own (with extra hooks);
the systemtest rule resolves whichever is registered via a toolchain type.
"""

SystemtestRunnerInfo = provider(
    doc = "The runner binary the systemtest rule invokes.",
    fields = {
        "binary": "File: the runner executable",
        "runfiles": "runfiles: the runner's runtime deps",
    },
)

def _systemtest_runner_impl(ctx):
    runfiles = ctx.attr.binary[DefaultInfo].default_runfiles
    return [
        DefaultInfo(runfiles = runfiles),
        SystemtestRunnerInfo(
            binary = ctx.executable.binary,
            runfiles = runfiles,
        ),
    ]

systemtest_runner = rule(
    implementation = _systemtest_runner_impl,
    attrs = {
        "binary": attr.label(
            mandatory = True,
            executable = True,
            cfg = "target",
            doc = "The runner executable.",
        ),
    },
    doc = "Wraps a runner binary for use as a systemtest runner toolchain.",
)

def _systemtest_runner_toolchain_impl(ctx):
    return [platform_common.ToolchainInfo(
        runner = ctx.attr.runner[SystemtestRunnerInfo],
    )]

systemtest_runner_toolchain = rule(
    implementation = _systemtest_runner_toolchain_impl,
    attrs = {
        "runner": attr.label(
            mandatory = True,
            providers = [SystemtestRunnerInfo],
        ),
    },
    doc = "A toolchain providing a systemtest runner.",
)
