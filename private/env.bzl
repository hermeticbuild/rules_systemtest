"""Naming of the environment variables a fixture's outputs are injected under.

The runner injects one variable per fixture output, prefixed by the uppercased,
sanitized fixture target name, so that several fixtures in one test stay
distinct. A fixture named `redis` exposing a port named `main` is seen by the
test as:

    REDIS_MAIN_HOST=127.0.0.1
    REDIS_MAIN_PORT=54231

The rule layer and the runner must agree on this transformation, so it lives
here rather than being open-coded in either.
"""

# Characters that survive into an environment variable name. Anything else
# (`-`, `.`, `/`, `+`, ...) becomes an underscore, since Bazel target names
# allow many characters that POSIX environment variable names do not.
_SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"

def env_prefix(fixture_name):
    """Returns the environment variable prefix for a fixture target name.

    Args:
        fixture_name: the fixture target's name, e.g. `redis-main`.

    Returns:
        The uppercased, sanitized prefix, e.g. `REDIS_MAIN`.
    """
    if not fixture_name:
        fail("fixture_name must not be empty")

    upper = fixture_name.upper()
    prefix = "".join([c if c in _SAFE else "_" for c in upper.elems()])

    # Environment variable names may not begin with a digit.
    if prefix[0].isdigit():
        prefix = "_" + prefix

    return prefix
