"""The coordinator toolchain: systemtest_coordinator + systemtest_coordinator_toolchain.

The coordinator is a normal binary the runner starts. It is resolved through a
toolchain so the ruleset can ship a precompiled one per platform while the
//src module can offer a from-source build of the same thing.

Remote mode (the coordinator already running elsewhere, reached over the
network) will add `endpoint`-shaped fields to SystemtestCoordinatorInfo; that is
the reason this is a provider behind a toolchain rather than a plain label flag.
"""

SystemtestCoordinatorInfo = provider(
    doc = "The coordinator binary the runner starts.",
    fields = {
        "binary": "File: the coordinator executable",
        "runfiles": "runfiles: the coordinator's runtime deps",
    },
)

def _systemtest_coordinator_impl(ctx):
    runfiles = ctx.attr.binary[DefaultInfo].default_runfiles
    return [
        DefaultInfo(runfiles = runfiles),
        SystemtestCoordinatorInfo(
            binary = ctx.executable.binary,
            runfiles = runfiles,
        ),
    ]

systemtest_coordinator = rule(
    implementation = _systemtest_coordinator_impl,
    attrs = {
        "binary": attr.label(
            mandatory = True,
            executable = True,
            cfg = "target",
            doc = "The coordinator executable.",
        ),
    },
    doc = "Wraps a coordinator binary for use as a systemtest coordinator toolchain.",
)

def _systemtest_coordinator_toolchain_impl(ctx):
    return [platform_common.ToolchainInfo(
        coordinator = ctx.attr.coordinator[SystemtestCoordinatorInfo],
    )]

systemtest_coordinator_toolchain = rule(
    implementation = _systemtest_coordinator_toolchain_impl,
    attrs = {
        "coordinator": attr.label(
            mandatory = True,
            providers = [SystemtestCoordinatorInfo],
        ),
    },
    doc = "A toolchain providing a systemtest coordinator.",
)
