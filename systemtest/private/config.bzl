"""Repository-level build settings for rules_systemtest.

  //:coordinator_mode — "local" (default) or "remote". Only "local" is wired
                        in the scaffold.

The flag itself is declared in the root BUILD.bazel via skylib's string_flag;
this module only holds the shared label/value constants the rules read so there
is a single source of truth.
"""

COORDINATOR_MODE_LOCAL = "local"
COORDINATOR_MODE_REMOTE = "remote"

COORDINATOR_MODE_FLAG = "//:coordinator_mode"
