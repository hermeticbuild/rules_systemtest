"""Unit tests for starlark helpers
See https://bazel.build/rules/testing#testing-starlark-utilities
"""

load("@bazel_skylib//lib:unittest.bzl", "asserts", "unittest")
load("//private:env.bzl", "env_prefix")

def _uppercases_impl(ctx):
    env = unittest.begin(ctx)
    asserts.equals(env, "REDIS", env_prefix("redis"))
    asserts.equals(env, "REDIS", env_prefix("REDIS"))
    return unittest.end(env)

def _sanitizes_impl(ctx):
    env = unittest.begin(ctx)
    asserts.equals(env, "REDIS_MAIN", env_prefix("redis-main"))
    asserts.equals(env, "REDIS_MAIN", env_prefix("redis.main"))
    asserts.equals(env, "MY_DB_V1", env_prefix("my_db+v1"))
    return unittest.end(env)

def _prefixes_leading_digit_impl(ctx):
    env = unittest.begin(ctx)
    asserts.equals(env, "_2ND_DB", env_prefix("2nd-db"))
    return unittest.end(env)

# The unittest library requires that we export the test cases as named test rules,
# but their names are arbitrary and don't appear anywhere.
_t0_test = unittest.make(_uppercases_impl)
_t1_test = unittest.make(_sanitizes_impl)
_t2_test = unittest.make(_prefixes_leading_digit_impl)

def env_test_suite(name):
    unittest.suite(name, _t0_test, _t1_test, _t2_test)
