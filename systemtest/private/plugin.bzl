"""The systemtest_plugin rule.

A plugin bundles one or more fixture-kind implementations behind one binary,
multiplexed by impl_id. In the scaffold only local mode (a `binary` the
coordinator starts) is exercised.
"""

SystemtestPluginInfo = provider(
    doc = "A systemtest plugin: the binary the coordinator starts and the impl_ids it serves.",
    fields = {
        "impl_ids": "list[str]: fixture kinds this plugin implements",
        "binary": "File: the plugin executable (local mode)",
        "container_ref": "str: digest-pinned image (remote mode; unused in scaffold)",
        "runfiles": "runfiles: the plugin binary's runtime deps",
    },
)

def _systemtest_plugin_impl(ctx):
    if not ctx.attr.binary and not ctx.attr.container_ref:
        fail("systemtest_plugin requires `binary` (local mode) or `container_ref` (remote mode).")

    binary_file = None
    runfiles = ctx.runfiles()
    if ctx.attr.binary:
        binary_file = ctx.executable.binary
        runfiles = runfiles.merge(ctx.attr.binary[DefaultInfo].default_runfiles)

    return [
        DefaultInfo(runfiles = runfiles),
        SystemtestPluginInfo(
            impl_ids = ctx.attr.impl_ids,
            binary = binary_file,
            container_ref = ctx.attr.container_ref,
            runfiles = runfiles,
        ),
    ]

systemtest_plugin = rule(
    implementation = _systemtest_plugin_impl,
    attrs = {
        "binary": attr.label(
            executable = True,
            cfg = "target",
            doc = "Local-mode plugin executable started by the coordinator.",
        ),
        "container_ref": attr.string(
            doc = "Remote-mode digest-pinned container ref (unused in scaffold).",
        ),
        "impl_ids": attr.string_list(
            mandatory = True,
            doc = "Fixture kinds this plugin implements.",
        ),
    },
    doc = "Declares a systemtest plugin.",
)
