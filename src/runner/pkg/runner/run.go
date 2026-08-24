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
// one lease per fixture, run the inner test binary with injected env, and
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
			code, err := execute(cmd.Context(), &runArgs{RunConfigPath: cfgPath})
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

// execute is Run's body, factored out for a plain (int, error) signature.
// The returned int is only meaningful for the inner test's own exit code;
// for setup failures before the test runs, callers should treat any
// non-nil error as exit code 1 (Run does this).
func execute(ctx context.Context, args *runArgs) (int, error) {
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

	startupTimeout := time.Duration(cfg.StartupTimeoutSeconds) * time.Second

	var leases []*activeLease
	defer func() {
		for _, l := range leases {
			releaseLease(client, l)
		}
	}()

	injectedEnv := map[string]string{}
	for _, fx := range fixtures {
		lease, err := acquireLease(ctx, client, fx, clientID, cfg.LeaseTTLSeconds, startupTimeout)
		if err != nil {
			return 0, err
		}
		leases = append(leases, lease)

		portEnv, err := portEnvFromOutputs(fx.Name, lease.Outputs)
		if err != nil {
			return 0, err
		}
		for k, v := range portEnv {
			injectedEnv[k] = v
		}
	}

	start := time.Now()
	exitCode, runErr := runTestBinary(testBinaryPath, cfg.Test.Args, injectedEnv, cfg.Test.Env)
	duration := time.Since(start)
	if runErr != nil {
		return 0, runErr
	}

	if err := writeJUnitReport(os.Getenv("XML_OUTPUT_FILE"), cfg.SystemtestLabel, exitCode, duration); err != nil {
		fmt.Fprintln(os.Stderr, "systemtest-runner: warning: writing junit report:", err)
	}

	return exitCode, nil
}
