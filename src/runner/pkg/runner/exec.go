package runner

import (
	"fmt"
	"os"
	"os/exec"
)

// buildTestCmd constructs the inner test command with stdio forwarded and a
// base env of os.Environ() + configEnv (configEnv wins on collision). Around
// hooks further decorate the returned command (e.g. the portenv hook appends
// fixture endpoint vars).
func buildTestCmd(binaryPath string, args []string, configEnv map[string]string) *exec.Cmd {
	cmd := exec.Command(binaryPath, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.Stdin = os.Stdin
	cmd.Env = mergeEnv(os.Environ(), configEnv)
	return cmd
}

// runCmd runs the (fully decorated) test command and returns its exit code.
func runCmd(cmd *exec.Cmd) (int, error) {
	if err := cmd.Run(); err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			return exitErr.ExitCode(), nil
		}
		return -1, fmt.Errorf("running test binary %s: %w", cmd.Path, err)
	}
	return 0, nil
}

// mergeEnv flattens base (as "K=V" strings) plus a sequence of overlays into
// a deduplicated env slice, with later overlays overriding earlier ones and
// base on the same key.
func mergeEnv(base []string, overlays ...map[string]string) []string {
	merged := map[string]string{}
	for _, kv := range base {
		for i := 0; i < len(kv); i++ {
			if kv[i] == '=' {
				merged[kv[:i]] = kv[i+1:]
				break
			}
		}
	}
	for _, overlay := range overlays {
		for k, v := range overlay {
			merged[k] = v
		}
	}
	out := make([]string, 0, len(merged))
	for k, v := range merged {
		out = append(out, k+"="+v)
	}
	return out
}
