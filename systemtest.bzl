"Public API re-exports"

load("//private:env.bzl", _env_prefix = "env_prefix")

# Exported so fixture authors can predict the environment variable names a
# fixture's outputs will be injected under.
env_prefix = _env_prefix
