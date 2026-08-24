"""The systemtest_fixture_rule factory and SystemtestFixtureInfo provider.

A fixture type is defined once by calling this factory at the top level of a
.bzl file; it returns a Bazel `rule` that is then instantiated in BUILD files.
"""

load(":plugin.bzl", "SystemtestPluginInfo")

SystemtestFixtureInfo = provider(
    doc = "A resolved fixture: its serialized description and runtime inputs.",
    fields = {
        "description_file": "File: <name>.fixture.json (protojson FixtureDescription)",
        "fixture_name": "str: target name; the env-var prefix for injected outputs",
        "runfiles": "runfiles: plugin binary + any file/image inputs",
    },
)

# Declared-input kind -> the attr type the generated rule exposes.
_SCALAR_KINDS = {
    "string": attr.string,
    "int": attr.int,
    "bool": attr.bool,
    "string_list": attr.string_list,
    "string_dict": attr.string_dict,
}

def _rlocation(ctx, file):
    """Runfiles rlocation key for a File in the merged runfiles tree."""
    if file.short_path.startswith("../"):
        return file.short_path[len("../"):]
    return ctx.workspace_name + "/" + file.short_path

def _value_json(kind, value):
    """Encode a declared input value as a systemtest.v1.Value (protojson dict)."""
    if kind in ("string_list", "string_dict"):
        return {"str": json.encode(value)}
    if kind == "bool":
        return {"str": "true" if value else "false"}
    if kind == "int":
        # Value.int_value is an int32; protojson renders it as a JSON number.
        return {"intValue": value}

    # string
    return {"str": value}

def _sharing_json(sharing):
    scope = sharing.get("scope", "client_restricted")
    max_conn = sharing.get("max_connections", "1")
    scope_key = {
        "unrestricted": "unrestricted",
        "client_restricted": "clientRestricted",
        "single_use": "singleUse",
    }.get(scope)
    if scope_key == None:
        fail("sharing scope must be one of unrestricted/client_restricted/single_use, got %r" % scope)
    return {
        scope_key: {},
        "maximumConcurrentConnections": max_conn,
    }

def _make_fixture_impl(ctx):
    plugin = ctx.attr.plugin[SystemtestPluginInfo]
    impl_id = ctx.attr._impl_id
    if impl_id not in plugin.impl_ids:
        fail("plugin %s does not implement impl_id %s (implements %s)" %
             (ctx.attr.plugin.label, impl_id, plugin.impl_ids))

    inputs_spec = json.decode(ctx.attr._inputs_spec)

    inputs = {}
    for name, kind in inputs_spec.items():
        if kind in ("file", "image"):
            # TODO(design: File-content inlining / Image upload): scaffold has no
            # file/image inputs; the runner would inline contents / push images.
            continue
        inputs[name] = _value_json(kind, getattr(ctx.attr, name))

    description = {
        "implId": impl_id,
        "binaryPath": _rlocation(ctx, plugin.binary) if plugin.binary else "",
        "inputs": inputs,
        "sharing": _sharing_json(ctx.attr.sharing),
        "name": ctx.label.name,
        "startupTimeoutSeconds": str(ctx.attr.startup_timeout_s),
    }

    out = ctx.actions.declare_file(ctx.label.name + ".fixture.json")
    ctx.actions.write(out, json.encode_indent(description, indent = "  "))

    runfiles = ctx.runfiles(files = [out]).merge(plugin.runfiles)
    return [
        DefaultInfo(files = depset([out]), runfiles = runfiles),
        SystemtestFixtureInfo(
            description_file = out,
            fixture_name = ctx.label.name,
            runfiles = runfiles,
        ),
    ]

def systemtest_fixture_rule(impl_id, inputs = {}):
    """Define a fixture type. Call at .bzl top level; bind the result to a global.

    Args:
      impl_id: the fixture-kind id; the plugin must list it in impl_ids.
      inputs: dict of input name -> kind (string/int/bool/string_list/
              string_dict/file/image).
    Returns:
      A Bazel rule.
    """
    rule_attrs = {
        "plugin": attr.label(
            mandatory = True,
            providers = [SystemtestPluginInfo],
            doc = "The systemtest_plugin implementing this fixture's impl_id.",
        ),
        "sharing": attr.string_dict(
            doc = "{'scope': ..., 'max_connections': 'N'}. Default client_restricted/1.",
        ),
        "startup_timeout_s": attr.int(default = 300),
        # Private attrs carry the factory's parameters into analysis.
        "_impl_id": attr.string(default = impl_id),
        "_inputs_spec": attr.string(default = json.encode(inputs)),
    }

    for name, kind in inputs.items():
        if kind in ("file", "image"):
            rule_attrs[name] = attr.label(allow_single_file = True)
        elif kind in _SCALAR_KINDS:
            rule_attrs[name] = _SCALAR_KINDS[kind]()
        else:
            fail("unknown input kind %r for %r" % (kind, name))

    return rule(
        implementation = _make_fixture_impl,
        attrs = rule_attrs,
        doc = "A systemtest fixture of kind %s." % impl_id,
    )
