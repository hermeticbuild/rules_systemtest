package runner

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// keepaliveInterval is how often a live lease is refreshed (CONTRACT.md step
// 6: "Keepalive every 15s").
const keepaliveInterval = 15 * time.Second

// defaultStartupTimeout bounds Status polling when a fixture description
// omits (or zeroes) startup_timeout_seconds.
const defaultStartupTimeout = 5 * time.Minute

// activeLease is a lease this runner holds, plus the machinery to keep it
// alive and release it.
type activeLease struct {
	ID          string
	Outputs     map[string]*systemtestpb.Value
	Credentials map[string]*systemtestpb.Value
	stop        context.CancelFunc
}

// fixtureFailedError is returned by acquireLease when a lease reaches FAILED.
// It carries the coordinator's high-level messages and the fixture's access
// credentials (if any), so the caller can dump debug info before giving up.
type fixtureFailedError struct {
	FixtureName string
	Status      systemtestpb.LeaseStatus
	Messages    []string
	Credentials map[string]*systemtestpb.Value
}

func (e *fixtureFailedError) Error() string {
	return fmt.Sprintf("fixture %q: lease %s: %s", e.FixtureName, e.Status, strings.Join(e.Messages, "; "))
}

// acquireLease calls TakeLease for one fixture, polling Status if the
// coordinator returns PENDING, and starts a background Keepalive loop once
// the lease is SUCCEEDED. On FAILED it returns a *fixtureFailedError carrying
// the messages and access credentials for debugging.
func acquireLease(
	ctx context.Context,
	client systemtestpb.CoordinatorServiceClient,
	fixture *resolvedFixture,
	clientID string,
	ttlSeconds int64,
	startupTimeout time.Duration,
) (*activeLease, error) {
	resp, err := client.TakeLease(ctx, &systemtestpb.TakeLeaseRequest{
		Fixture:              fixture.Description,
		ClientId:             clientID,
		Owner:                clientID,
		TtlSeconds:           ttlSeconds,
		AssociatedTestsCount: 1,
	})
	if err != nil {
		return nil, fmt.Errorf("fixture %q: TakeLease: %w", fixture.Name, err)
	}

	status, leaseID := resp.GetStatus(), resp.GetLeaseId()
	outputs, creds, messages := resp.GetOutputs(), resp.GetCredentials(), resp.GetMessages()

	if status == systemtestpb.LeaseStatus_PENDING {
		if startupTimeout <= 0 {
			startupTimeout = defaultStartupTimeout
		}
		pollCtx, cancel := context.WithTimeout(ctx, startupTimeout)
		defer cancel()
		status, outputs, creds, messages, err = pollUntilDone(pollCtx, client, leaseID)
		if err != nil {
			return nil, fmt.Errorf("fixture %q: waiting for lease: %w", fixture.Name, err)
		}
	}

	if status != systemtestpb.LeaseStatus_SUCCEEDED {
		return nil, &fixtureFailedError{
			FixtureName: fixture.Name,
			Status:      status,
			Messages:    messages,
			Credentials: creds,
		}
	}

	leaseCtx, cancel := context.WithCancel(context.Background())
	go keepaliveLoop(leaseCtx, client, leaseID)

	return &activeLease{ID: leaseID, Outputs: outputs, Credentials: creds, stop: cancel}, nil
}

// pollUntilDone polls Status every 2s until the lease reaches a terminal
// state (SUCCEEDED/FAILED) or ctx is done.
func pollUntilDone(
	ctx context.Context,
	client systemtestpb.CoordinatorServiceClient,
	leaseID string,
) (systemtestpb.LeaseStatus, map[string]*systemtestpb.Value, map[string]*systemtestpb.Value, []string, error) {
	ticker := time.NewTicker(2 * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return systemtestpb.LeaseStatus_LEASE_STATUS_UNKNOWN, nil, nil, nil, ctx.Err()
		case <-ticker.C:
			resp, err := client.Status(ctx, &systemtestpb.StatusRequest{LeaseId: leaseID})
			if err != nil {
				return systemtestpb.LeaseStatus_LEASE_STATUS_UNKNOWN, nil, nil, nil, fmt.Errorf("Status: %w", err)
			}
			switch resp.GetStatus() {
			case systemtestpb.LeaseStatus_SUCCEEDED, systemtestpb.LeaseStatus_FAILED:
				return resp.GetStatus(), resp.GetOutputs(), resp.GetCredentials(), resp.GetMessages(), nil
			}
			// PENDING (or UNKNOWN): keep polling.
		}
	}
}

// keepaliveLoop sends Keepalive every keepaliveInterval until ctx is
// cancelled (by releaseLease). Keepalive failures are logged, not fatal:
// losing one heartbeat shouldn't fail a running test.
func keepaliveLoop(ctx context.Context, client systemtestpb.CoordinatorServiceClient, leaseID string) {
	ticker := time.NewTicker(keepaliveInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			_, _ = client.Keepalive(ctx, &systemtestpb.KeepaliveRequest{LeaseId: leaseID})
		}
	}
}

// releaseLease stops the keepalive loop and calls ReleaseLease. Errors are
// best-effort: the coordinator will eventually GC an unreleased lease on TTL
// expiry, and we're already on the way out.
func releaseLease(client systemtestpb.CoordinatorServiceClient, lease *activeLease) {
	lease.stop()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, _ = client.ReleaseLease(ctx, &systemtestpb.ReleaseRequest{LeaseId: lease.ID})
}
