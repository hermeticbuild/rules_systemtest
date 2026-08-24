// Command systemtest-runner is the launcher-invoked entrypoint for
// rules_systemtest. It has a single subcommand, `run`, and a single flag,
// `--run-config`, per the frozen contract in .context/CONTRACT.md.
package main

import (
	"os"

	"github.com/hermeticbuild/rules_systemtest/src/runner/pkg/runner"
)

func main() {
	os.Exit(runner.Run(os.Args))
}
