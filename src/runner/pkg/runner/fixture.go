package runner

import (
	"fmt"
	"os"

	"google.golang.org/protobuf/encoding/protojson"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// resolvedFixture pairs a fixture's config-file name with its parsed
// description.
type resolvedFixture struct {
	Name        string
	Description *systemtestpb.FixtureDescription
}

// loadFixtureDescription resolves a fixture's description_path via runfiles
// and unmarshals the protojson FixtureDescription found there. The
// description is forwarded to the coordinator verbatim apart from
// binary_path, which is rewritten to an absolute path: the coordinator is a
// long-lived daemon with no runfiles tree of its own, so the runner — the
// only process that knows this test's runfiles — must resolve it.
func loadFixtureDescription(fc FixtureConfig) (*resolvedFixture, error) {
	path, err := resolveRlocation(fc.DescriptionPath)
	if err != nil {
		return nil, fmt.Errorf("fixture %q: %w", fc.Name, err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("fixture %q: reading description %s: %w", fc.Name, path, err)
	}
	var desc systemtestpb.FixtureDescription
	if err := protojson.Unmarshal(data, &desc); err != nil {
		return nil, fmt.Errorf("fixture %q: parsing description %s: %w", fc.Name, path, err)
	}
	if rl := desc.GetBinaryPath(); rl != "" {
		resolved, err := resolveRlocation(rl)
		if err != nil {
			return nil, fmt.Errorf("fixture %q: binary_path: %w", fc.Name, err)
		}
		desc.Plugin = &systemtestpb.FixtureDescription_BinaryPath{BinaryPath: resolved}
	}
	return &resolvedFixture{Name: fc.Name, Description: &desc}, nil
}
