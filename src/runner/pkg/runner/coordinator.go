package runner

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

const socketWaitTimeout = 10 * time.Second

// socketDir returns the per-workspace directory holding the coordinator's UDS
// and lock file: $TMPDIR/systemtest/<hash-of-workspace-key>. $TMPDIR, not the
// action's temp dir, so the daemon and its socket outlive the test action that
// started it.
func socketDir(wsKey string) string {
	sum := sha256.Sum256([]byte(wsKey))
	return filepath.Join(os.TempDir(), "systemtest", hex.EncodeToString(sum[:])[:16])
}

// ensureCoordinatorRunning returns the path of a UDS an already-running
// coordinator daemon or launches a new one.
//
// No locking here: the coordinator itself enforces that only one instance
// serves a socket dir, and a spawn that loses that race exits quietly. So the
// worst case for concurrent runners is a few short-lived processes, and this
// stays a probe-then-spawn.
func ensureCoordinatorRunning(ctx context.Context, coordinatorBin, wsKey string) (string, error) {
	dir := socketDir(wsKey)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", fmt.Errorf("creating socket dir %s: %w", dir, err)
	}
	sockPath := filepath.Join(dir, "server.sock")

	if socketAccepts(sockPath) {
		return sockPath, nil
	}

	if err := spawnCoordinator(coordinatorBin, wsKey, sockPath); err != nil {
		return "", err
	}
	if err := waitForSocket(ctx, sockPath, socketWaitTimeout); err != nil {
		return "", err
	}
	return sockPath, nil
}

// socketAccepts reports whether a coordinator is listening at path. It dials
// rather than stat-ing: a socket file left behind by a daemon that died is
// still on disk, and treating that as "running" would skip the spawn and then
// fail to connect.
func socketAccepts(path string) bool {
	conn, err := net.DialTimeout("unix", path, 200*time.Millisecond)
	if err != nil {
		return false
	}
	conn.Close()
	return true
}

// spawnCoordinator starts the coordinator binary detached (its own session)
// so it outlives this runner invocation, then returns immediately; readiness
// is confirmed separately by waitForSocket.
func spawnCoordinator(coordinatorBin, wsKey, sockPath string) error {
	cmd := exec.Command(coordinatorBin, "--workspace", wsKey, "--socket", sockPath)
	// nil Stdout/Stderr connect to /dev/null: the daemon outlives us and must
	// not block on a pipe no one is reading, or inherit our terminal.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("starting coordinator %s: %w", coordinatorBin, err)
	}
	go cmd.Wait() // reap it; we don't care about its exit status
	return nil
}

// waitForSocket polls until a UDS at path accepts a connection or timeout
// elapses.
func waitForSocket(ctx context.Context, path string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var lastErr error
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("unix", path, 200*time.Millisecond)
		if err == nil {
			conn.Close()
			return nil
		}
		lastErr = err
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
	return fmt.Errorf("timed out after %s waiting for coordinator socket %s: %w", timeout, path, lastErr)
}

// dialCoordinator opens a gRPC connection to the coordinator over its UDS.
// The coordinator (a Rust/tonic server) speaks plaintext HTTP/2 (h2c) on the
// socket; insecure credentials plus a custom unix dialer interop with that
// without TLS.
func dialCoordinator(sockPath string) (*grpc.ClientConn, error) {
	dialer := func(ctx context.Context, _ string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "unix", sockPath)
	}
	conn, err := grpc.NewClient(
		"unix:"+sockPath,
		grpc.WithContextDialer(dialer),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		return nil, fmt.Errorf("dialing coordinator at %s: %w", sockPath, err)
	}
	return conn, nil
}
