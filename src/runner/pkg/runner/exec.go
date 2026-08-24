package runner

import (
	"fmt"
	"os"
	"os/exec"
)

// runTestBinary execs the resolved inner test binary, forwarding stdio, and
// returns its exit code. env is layered as os.Environ() + injectedEnv +
// configEnv (later layers win on key collisions), per CONTRACT.md step 8.
func runTestBinary(binaryPath string, args []string, injectedEnv, configEnv map[string]string) (int, error) {
	cmd := exec.Command(binaryPath, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.Stdin = os.Stdin
	cmd.Env = mergeEnv(os.Environ(), injectedEnv, configEnv)

	if err := cmd.Run(); err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			return exitErr.ExitCode(), nil
		}
		return -1, fmt.Errorf("running test binary %s: %w", binaryPath, err)
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
