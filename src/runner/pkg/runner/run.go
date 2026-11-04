package runner

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/spf13/cobra"

	"github.com/hermeticbuild/rules_systemtest/src/proto/systemtestpb"
)

// Run is the systemtest-runner entrypoint: parse args, resolve config, take
// one lease per fixture, run the inner test binary (with the stock hooks), and
// report results. It never panics out; all errors are printed to stderr and
// turned into a process exit code, which is what callers (main.go) should
// pass to os.Exit.
func Run(argv []string) int {
	var cfgPath string
	var exitCode int
	ranRun := false

	runCmd := &cobra.Command{
		Use:   "run",
		Short: "Lease the configured fixtures and run the inner test binary",
		Args:  cobra.NoArgs,
		// We format and route errors ourselves (below) to keep the inner
		// test's exit code distinct from usage/setup failures.
		SilenceUsage:  true,
		SilenceErrors: true,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ranRun = true
			code, err := defaultRunner().execute(cmd.Context(), &RunArgs{RunConfigPath: cfgPath})
			exitCode = code
			return err
		},
	}
	runCmd.Flags().StringVar(&cfgPath, "run-config", "",
		"rlocation path to run_config.json")
	_ = runCmd.MarkFlagRequired("run-config")

	root := &cobra.Command{
		Use:           "systemtest-runner",
		Short:         "rules_systemtest test launcher entrypoint",
		SilenceUsage:  true,
		SilenceErrors: true,
	}
	root.AddCommand(runCmd)
	root.SetArgs(argv[1:])

	if err := root.ExecuteContext(context.Background()); err != nil {
		fmt.Fprintln(os.Stderr, "systemtest-runner:", err)
		if !ranRun {
			// Flag/usage error before the command body ran.
			return 2
		}
		if exitCode == 0 {
			exitCode = 1
		}
	}
	return exitCode
}

// execute is the hook-driven run body. The returned int is only meaningful for
// the inner test's own exit code; for setup failures before the test runs,
// callers treat any non-nil error as exit code 1.
func (r *Runner) execute(ctx context.Context, args *RunArgs) (int, error) {
	cfg, err := loadRunConfig(args.RunConfigPath)
	if err != nil {
		return 0, err
	}

	coordinatorBinPath, err := resolveRlocation(cfg.CoordinatorBin)
	if err != nil {
		return 0, fmt.Errorf("resolving coordinator_bin: %w", err)
	}
	testBinaryPath, err := resolveRlocation(cfg.Test.BinaryPath)
	if err != nil {
		return 0, fmt.Errorf("resolving test.binary_path: %w", err)
	}

	fixtures := make([]*resolvedFixture, 0, len(cfg.Fixtures))
	for _, fc := range cfg.Fixtures {
		rf, err := loadFixtureDescription(fc)
		if err != nil {
			return 0, err
		}
		fixtures = append(fixtures, rf)
	}

	wsKey, err := workspaceKey()
	if err != nil {
		return 0, err
	}
	clientID := computeClientID(wsKey)

	sockPath, err := ensureCoordinatorRunning(ctx, coordinatorBinPath, wsKey)
	if err != nil {
		return 0, fmt.Errorf("starting coordinator: %w", err)
	}

	conn, err := dialCoordinator(sockPath)
	if err != nil {
		return 0, fmt.Errorf("dialing coordinator: %w", err)
	}
	defer conn.Close()
	client := systemtestpb.NewCoordinatorServiceClient(conn)

	hctx := HookCtx{Context: ctx, Args: *args, Log: stderrLogger{}}

	// PreHooks: before any lease; may mutate the fixture description.
	for _, fx := range fixtures {
		for _, h := range r.pre {
			if err := h.BeforeLease(hctx, fx.Description); err != nil {
				return 0, fmt.Errorf("fixture %q: pre-hook %s: %w", fx.Name, h.Name(), err)
			}
		}
	}

	startupTimeout := time.Duration(cfg.StartupTimeoutSeconds) * time.Second

	var leases []*activeLease
	defer func() {
		for _, l := range leases {
			releaseLease(client, l)
		}
	}()

	hookLeases := make([]*Lease, 0, len(fixtures))
	for _, fx := range fixtures {
		al, err := acquireLease(ctx, client, fx, clientID, cfg.LeaseTTLSeconds, startupTimeout)
		if err != nil {
			return 0, err
		}
		leases = append(leases, al)
		hookLeases = append(hookLeases, &Lease{
			LeaseID:     al.ID,
			FixtureName: fx.Name,
			Outputs:     al.Outputs,
		})
	}

	// AroundHooks: each (lease × hook) returns a Wrapping that decorates the
	// test command. Cleanups run in reverse (registered after the release
	// defer, so they run before leases are released).
	cmd := buildTestCmd(testBinaryPath, cfg.Test.Args, cfg.Test.Env)
	var cleanups []func() error
	defer func() {
		for i := len(cleanups) - 1; i >= 0; i-- {
			if cerr := cleanups[i](); cerr != nil {
				fmt.Fprintln(os.Stderr, "systemtest-runner: around-hook cleanup:", cerr)
			}
		}
	}()
	for _, lease := range hookLeases {
		for _, h := range r.around {
			w, err := h.Setup(hctx, lease)
			if err != nil {
				return 0, fmt.Errorf("fixture %q: around-hook %s: %w", lease.FixtureName, h.Name(), err)
			}
			if w.Wrap != nil {
				cmd = w.Wrap(cmd)
			}
			if w.Cleanup != nil {
				cleanups = append(cleanups, w.Cleanup)
			}
		}
	}

	start := time.Now()
	exitCode, runErr := runCmd(cmd)
	duration := time.Since(start)
	if runErr != nil {
		return 0, runErr
	}
	result := TestResult{ExitCode: exitCode, Duration: duration}

	// PostHooks: after the test, before leases are released.
	for _, lease := range hookLeases {
		for _, h := range r.post {
			if err := h.AfterTest(hctx, lease, result); err != nil {
				fmt.Fprintf(os.Stderr, "systemtest-runner: post-hook %s: %v\n", h.Name(), err)
			}
		}
	}

	if err := writeJUnitReport(os.Getenv("XML_OUTPUT_FILE"), cfg.SystemtestLabel, exitCode, duration); err != nil {
		fmt.Fprintln(os.Stderr, "systemtest-runner: warning: writing junit report:", err)
	}

	return exitCode, nil
}
