package runner

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strconv"
	"strings"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// portsSchemaID is the schema_id a fixture output's SchemaBlob must carry for
// the runner to treat it as port endpoints (see CONTRACT.md "Env
// injection").
const portsSchemaID = "systemtest.ports"

// portsSchema mirrors the JSON contents of a systemtest.ports output.
type portsSchema struct {
	Ports []portEntry `json:"ports"`
}

type portEntry struct {
	Name     string `json:"name"`
	Protocol string `json:"protocol"`
	Tunnel   bool   `json:"tunnel"`
	Host     string `json:"host"`
	Port     int    `json:"port"`
}

// nonAlnum matches runs of characters that aren't ASCII letters or digits,
// for sanitizing fixture/port names into env-var name fragments.
var nonAlnum = regexp.MustCompile(`[^A-Za-z0-9]+`)

// envFragment uppercases s and collapses every run of non-alphanumeric
// characters to a single underscore, matching CONTRACT.md's "uppercased,
// non-alnum -> '_' sanitized" rule for both the fixture-name prefix and each
// port name.
func envFragment(s string) string {
	return strings.ToUpper(nonAlnum.ReplaceAllString(s, "_"))
}

// portEnvFromOutputs scans one fixture's TakeLease outputs for
// systemtest.ports blobs and returns the <PREFIX>_<NAME>_HOST/PORT env vars
// for every non-tunneled port. Tunneled ports are out of scope for the
// scaffold (no portforward hook).
func portEnvFromOutputs(fixtureName string, outputs map[string]*systemtestpb.Value) (map[string]string, error) {
	env := map[string]string{}
	prefix := envFragment(fixtureName)
	for outputName, v := range outputs {
		blob := v.GetSchemaBlob()
		if blob == nil || blob.GetSchemaId() != portsSchemaID {
			continue
		}
		var parsed portsSchema
		if err := json.Unmarshal(blob.GetContents(), &parsed); err != nil {
			return nil, fmt.Errorf("fixture %q output %q: parsing systemtest.ports: %w", fixtureName, outputName, err)
		}
		for _, p := range parsed.Ports {
			if p.Tunnel {
				continue // scaffold: only tunnel:false is handled
			}
			portName := envFragment(p.Name)
			env[fmt.Sprintf("%s_%s_HOST", prefix, portName)] = p.Host
			env[fmt.Sprintf("%s_%s_PORT", prefix, portName)] = strconv.Itoa(p.Port)
		}
	}
	return env, nil
}
