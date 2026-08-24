"""High-level macros that hide the fixture-type definition + built-in plugin."""

load(":builtin_fixtures.bzl", "dummy")

def dummy_fixture(name, plugin, ports = None, replicas = None, sharing = None, **kwargs):
    """A fixture that provisions nothing. Scaffold demo of the full plumbing.

    Args:
      name: target name; the env-var prefix for injected endpoints.
      plugin: the systemtest_plugin implementing the `dummy` impl_id. No
              default: the reference plugin is built from source in the
              rules_systemtest_src module, which this module cannot name
              (see the module comment in //:MODULE.bazel), so it has to be
              passed in — e.g. "@rules_systemtest_src//plugins/dummy".
      ports: {logical_name: "port"} to echo back as endpoints, e.g. {"main": "6379"}.
      replicas: an integer input (unused; demonstrates the int input path).
      sharing: {"scope": ..., "max_connections": "N"}.
      **kwargs: forwarded (e.g. startup_timeout_s, visibility, tags).
    """
    dummy(
        name = name,
        ports = ports or {},
        replicas = replicas or 0,
        plugin = plugin,
        sharing = sharing or {},
        **kwargs
    )
