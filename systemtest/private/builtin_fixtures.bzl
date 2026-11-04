"""Built-in fixture types, defined via the fixture-rule factory.
"""

load(":fixture.bzl", "systemtest_fixture_rule")

dummy = systemtest_fixture_rule(
    impl_id = "@rules_systemtest//fixtures:dummy",
    inputs = {
        "ports": "string_dict",
        "replicas": "int",
        # Optional JSON file of extra {name: port} entries, merged into `ports`.
        # Demonstrates the `file` input kind (contents inlined by the runner).
        "config": "file",
    },
)
