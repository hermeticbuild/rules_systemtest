"""Public API for rules_systemtest.

    load("@rules_systemtest//systemtest:defs.bzl", "systemtest", "dummy_fixture", ...)
"""

load("//systemtest/private:builtin_fixtures.bzl", _dummy = "dummy")
load(
    "//systemtest/private:coordinator.bzl",
    _SystemtestCoordinatorInfo = "SystemtestCoordinatorInfo",
    _systemtest_coordinator = "systemtest_coordinator",
    _systemtest_coordinator_toolchain = "systemtest_coordinator_toolchain",
)
load("//systemtest/private:env.bzl", _env_prefix_for_fixture = "env_prefix_for_fixture")
load(
    "//systemtest/private:fixture.bzl",
    _SystemtestFixtureInfo = "SystemtestFixtureInfo",
    _systemtest_fixture_rule = "systemtest_fixture_rule",
)
load("//systemtest/private:macros.bzl", _dummy_fixture = "dummy_fixture")
load(
    "//systemtest/private:plugin.bzl",
    _SystemtestPluginInfo = "SystemtestPluginInfo",
    _systemtest_plugin = "systemtest_plugin",
)
load(
    "//systemtest/private:runner.bzl",
    _SystemtestRunnerInfo = "SystemtestRunnerInfo",
    _systemtest_runner = "systemtest_runner",
    _systemtest_runner_toolchain = "systemtest_runner_toolchain",
)
load("//systemtest/private:systemtest.bzl", _systemtest = "systemtest")

# Core rules
systemtest = _systemtest
systemtest_fixture_rule = _systemtest_fixture_rule
systemtest_plugin = _systemtest_plugin

# Toolchains: for supplying your own runner (extra hooks) or coordinator, or
# building the shipped ones from source. See //systemtest/toolchains for the types.
systemtest_runner = _systemtest_runner
systemtest_runner_toolchain = _systemtest_runner_toolchain
systemtest_coordinator = _systemtest_coordinator
systemtest_coordinator_toolchain = _systemtest_coordinator_toolchain

# High-level macros
dummy_fixture = _dummy_fixture

# Built-in fixture types
dummy = _dummy

# Exported so fixture authors can predict the environment variable names a
# fixture's outputs will be injected under.
env_prefix_for_fixture = _env_prefix_for_fixture

# Providers
SystemtestFixtureInfo = _SystemtestFixtureInfo
SystemtestPluginInfo = _SystemtestPluginInfo
SystemtestRunnerInfo = _SystemtestRunnerInfo
SystemtestCoordinatorInfo = _SystemtestCoordinatorInfo
