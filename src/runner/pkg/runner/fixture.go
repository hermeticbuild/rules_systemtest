package runner

import (
	"encoding/json"
	"fmt"
	"os"
	"path"

	"google.golang.org/protobuf/encoding/protojson"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// resolvedFixture pairs a fixture's config-file name with its parsed
// description.
type resolvedFixture struct {
	Name        string
	Description *systemtestpb.FixtureDescription
}

// fileListEntry is one member of a `file_list` input after inlining: the
// original base name plus the file's contents.
type fileListEntry struct {
	Name     string `json:"name"`
	Contents string `json:"contents"`
}

// loadFixtureDescription resolves a fixture's description_path via runfiles
// and unmarshals the protojson FixtureDescription found there. The
// description is forwarded to the coordinator verbatim apart from
// binary_path (rewritten to an absolute path, since the coordinator daemon
// has no runfiles tree of its own) and file inputs (whose contents are
// inlined here, the only process that knows this test's runfiles).
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
	if err := inlineFileInputs(fc.Name, &desc); err != nil {
		return nil, err
	}
	return &resolvedFixture{Name: fc.Name, Description: &desc}, nil
}

// inlineFileInputs replaces each file_input's rlocation path(s) with the
// file contents, before the description reaches the coordinator
// (CreateRequest.inputs is documented as already-inlined).
//
//   - a `file` input is a Value.str holding one rlocation; it becomes a
//     Value.data of the file's raw bytes.
//   - a `file_list` input is a Value.str holding a JSON array of rlocations;
//     it becomes a Value.str holding a JSON array of {name, contents}.
//
// The two are told apart by whether the current Value.str parses as a JSON
// array of strings (a single rlocation path never does).
func inlineFileInputs(fixtureName string, desc *systemtestpb.FixtureDescription) error {
	for _, key := range desc.GetFileInputs() {
		v := desc.GetInputs()[key]
		if v == nil {
			return fmt.Errorf("fixture %q: file_input %q missing from inputs", fixtureName, key)
		}
		raw := v.GetStr()

		var paths []string
		if err := json.Unmarshal([]byte(raw), &paths); err == nil {
			entries := make([]fileListEntry, 0, len(paths))
			for _, rl := range paths {
				contents, err := readRlocation(fixtureName, key, rl)
				if err != nil {
					return err
				}
				entries = append(entries, fileListEntry{Name: path.Base(rl), Contents: string(contents)})
			}
			blob, err := json.Marshal(entries)
			if err != nil {
				return fmt.Errorf("fixture %q: encoding file_list %q: %w", fixtureName, key, err)
			}
			v.Value = &systemtestpb.Value_Str{Str: string(blob)}
			continue
		}

		contents, err := readRlocation(fixtureName, key, raw)
		if err != nil {
			return err
		}
		v.Value = &systemtestpb.Value_Data{Data: contents}
	}
	return nil
}

func readRlocation(fixtureName, key, rl string) ([]byte, error) {
	resolved, err := resolveRlocation(rl)
	if err != nil {
		return nil, fmt.Errorf("fixture %q: file_input %q: %w", fixtureName, key, err)
	}
	contents, err := os.ReadFile(resolved)
	if err != nil {
		return nil, fmt.Errorf("fixture %q: file_input %q: reading %s: %w", fixtureName, key, resolved, err)
	}
	return contents, nil
}
