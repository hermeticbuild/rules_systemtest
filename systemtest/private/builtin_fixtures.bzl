"""Built-in fixture types, defined via the fixture-rule factory.
"""

load(":fixture.bzl", "systemtest_fixture_rule")

dummy = systemtest_fixture_rule(
    impl_id = "@rules_systemtest//fixtures:dummy",
    inputs = {
        "ports": "string_dict",
        "replicas": "int",
    },
)
