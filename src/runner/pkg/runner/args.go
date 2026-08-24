package runner

// runArgs is the parsed `run` subcommand input: `systemtest-runner run
// --run-config <rlocation path>`. This is the only subcommand/flag the
// contract defines.
type runArgs struct {
	// RunConfigPath is a runfiles-relative (rlocation) path to run_config.json.
	RunConfigPath string
}
