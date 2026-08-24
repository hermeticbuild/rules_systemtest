package runner

import (
	"fmt"

	"github.com/bazelbuild/rules_go/go/runfiles"
)

// resolveRlocation resolves a runfiles-relative (rlocation) path to an
// absolute filesystem path. Every path the runner is handed (in the config,
// in flags) is an rlocation path per the contract; nothing is a bare
// filesystem path.
func resolveRlocation(path string) (string, error) {
	resolved, err := runfiles.Rlocation(path)
	if err != nil {
		return "", fmt.Errorf("resolving rlocation %q: %w", path, err)
	}
	return resolved, nil
}
