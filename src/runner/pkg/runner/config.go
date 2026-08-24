package runner

import (
	"encoding/json"
	"fmt"
	"os"
)

// FixtureConfig is one entry of run_config.json's "fixtures" list.
type FixtureConfig struct {
	Name            string `json:"name"`
	DescriptionPath string `json:"description_path"`
}

// TestConfig is run_config.json's "test" object: the inner test binary and
// how to invoke it.
type TestConfig struct {
	BinaryPath string            `json:"binary_path"`
	Args       []string          `json:"args"`
	Env        map[string]string `json:"env"`
}

// RunConfig is the run_config.json schema written by the systemtest Starlark
// rule. All paths are rlocation paths.
type RunConfig struct {
	SystemtestLabel       string          `json:"systemtest_label"`
	Mode                  string          `json:"mode"`
	CoordinatorBin        string          `json:"coordinator_bin"`
	LeaseTTLSeconds       int64           `json:"lease_ttl_seconds"`
	StartupTimeoutSeconds int64           `json:"startup_timeout_seconds"`
	Fixtures              []FixtureConfig `json:"fixtures"`
	Test                  TestConfig      `json:"test"`
}

// loadRunConfig loads the run_config.json passed in as an arg.
func loadRunConfig(rlocationPath string) (*RunConfig, error) {
	path, err := resolveRlocation(rlocationPath)
	if err != nil {
		return nil, fmt.Errorf("resolving --run-config: %w", err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("reading run config %s: %w", path, err)
	}
	var cfg RunConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("parsing run config %s: %w", path, err)
	}
	return &cfg, nil
}
