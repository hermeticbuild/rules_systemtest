package runner

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"os/user"
	"path/filepath"

	"github.com/google/uuid"
)

// workspaceKey identifies the workspace this invocation belongs to. It keys
// both the coordinator's socket dir and the fixture-reuse client_id, so every
// test action in one workspace has to agree on it -- otherwise each action gets
// its own coordinator, and with it its own copy of every fixture.
//
// Nothing in a test action's environment names the source root:
// BUILD_WORKSPACE_DIRECTORY is only set by `bazel run`, TEST_WORKSPACE is a
// workspace *name* (which collides between two checkouts), and every path
// points into the per-action sandbox. So this binary stands in as the proxy: the
// runner in runfiles is a symlink into the real output tree, and resolving it
// escapes the sandbox to a path that is stable per workspace.
func workspaceKey() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("locating runner binary: %w", err)
	}
	resolved, err := filepath.EvalSymlinks(exe)
	if err != nil {
		return "", fmt.Errorf("resolving runner binary %s: %w", exe, err)
	}
	return resolved, nil
}

// computeClientID derives the coordinator's fixture-reuse key.
// It must be stable across runs of the same user/workspace (so reuse works) and
// distinct across users/checkouts (so isolation holds).
func computeClientID(wsKey string) string {
	// TODO: have some way of injecting additional discriminators for running in
	// CI where different pipelines reuse the same host/user.
	username := "unknown"
	if u, err := user.Current(); err == nil && u.Username != "" {
		username = u.Username
	}
	h := sha256.New()
	// The host, via its MAC address. NOTE: uuid.NodeID() falls back to a
	// per-process random value on a machine with no usable interface, which
	// would make this id -- and so fixture reuse -- unstable.
	h.Write(uuid.NodeID())
	h.Write([]byte{0})
	h.Write([]byte(username))
	h.Write([]byte{0})
	h.Write([]byte(wsKey))
	return hex.EncodeToString(h.Sum(nil))
}
