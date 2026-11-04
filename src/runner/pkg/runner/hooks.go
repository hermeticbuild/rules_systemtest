package runner

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"time"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// Runner-package aliases for the proto types the hook API speaks in, so hook
// signatures read as DESIGN.md specifies them.
type (
	FixtureDescription = systemtestpb.FixtureDescription
	Value              = systemtestpb.Value
)

// Logger is the minimal logging surface handed to hooks.
type Logger interface {
	Printf(format string, args ...any)
}

type stderrLogger struct{}

func (stderrLogger) Printf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "systemtest-runner: "+format+"\n", args...)
}

// HookCtx is passed to every hook invocation.
type HookCtx struct {
	Context context.Context
	Args    RunArgs
	Log     Logger
}

// Lease is the resolved fixture a hook operates on. Outputs are schema-tagged
// Values forwarded verbatim from the plugin.
type Lease struct {
	LeaseID     string
	FixtureName string // the fixture target name; the env-var prefix
	Outputs     map[string]*Value
}

// TestResult is handed to PostHooks after the inner test exits.
type TestResult struct {
	ExitCode int
	Duration time.Duration
}

// Wrapping decorates the test command. Cleanups run in reverse (defer-style);
// a non-nil Cleanup error fails the test.
type Wrapping struct {
	Wrap    func(*exec.Cmd) *exec.Cmd
	Cleanup func() error
}

// PreHook runs before TakeLease and may mutate the fixture description. Runs
// in registration order.
type PreHook interface {
	Name() string
	BeforeLease(HookCtx, *FixtureDescription) error
}

// AroundHook brackets test execution. Setup returns a Wrapping that decorates
// the test command.
type AroundHook interface {
	Name() string
	Setup(HookCtx, *Lease) (Wrapping, error)
}

// PostHook runs after the test exits, before ReleaseLease.
type PostHook interface {
	Name() string
	AfterTest(HookCtx, *Lease, TestResult) error
}

// Runner holds the hook chains and drives a systemtest invocation.
type Runner struct {
	pre    []PreHook
	around []AroundHook
	post   []PostHook
}

// New returns a Runner with no hooks.
func New() *Runner { return &Runner{} }

func (r *Runner) WithPreHook(h PreHook) *Runner {
	r.pre = append(r.pre, h)
	return r
}

func (r *Runner) WithAroundHook(h AroundHook) *Runner {
	r.around = append(r.around, h)
	return r
}

func (r *Runner) WithPostHook(h PostHook) *Runner {
	r.post = append(r.post, h)
	return r
}

// defaultRunner is the runner the stock `systemtest-runner` binary uses: the
// one built-in behavior is injecting endpoint env from systemtest.ports
// outputs, now expressed as an Around hook so there is a single code path.
func defaultRunner() *Runner {
	return New().WithAroundHook(portEnvHook{})
}

// Run executes with this runner's hooks, mapping errors to a process exit
// code. This is the entry point for custom runners built via New().With...().
func (r *Runner) Run(ctx context.Context, args RunArgs) int {
	code, err := r.execute(ctx, &args)
	if err != nil {
		stderrLogger{}.Printf("%v", err)
		if code != 0 {
			return code
		}
		return 1
	}
	return code
}
