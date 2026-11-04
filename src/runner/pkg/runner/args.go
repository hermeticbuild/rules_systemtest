package runner

// RunArgs is the parsed `run` subcommand input: `systemtest-runner run
// --run-config <rlocation path>`. This is the only subcommand/flag the
// contract defines. It is also handed to hooks via HookCtx.
type RunArgs struct {
	// RunConfigPath is a runfiles-relative (rlocation) path to run_config.json.
	RunConfigPath string
}
