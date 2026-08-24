# rules_systemtest — design

`rules_systemtest` is a Bazel ruleset (plus a coordinator service, a test runner, and a
plugin protocol) for integration tests: tests that need one or more external services —
a database, a message broker, your own service under test — running before and during the
test.

It provides three things:

- **A Bazel rule layer** for declaring *fixtures* (the external services a test depends on)
  and wiring them to tests.
- **A coordinator** — a long-running service that owns fixture lifecycle, reuse, leases, and
  resource quota.
- **A test runner** with composable hooks (image upload, port-forwarding, log capture) that
  brackets the user's test process.

## Why

Bazel can cache and reproduce unit tests, but integration tests depend on running services
Bazel can neither start, cache, nor reproduce. The common workaround — disable caching and
stand services up in CI before the test — makes pipelines slow and flaky.

`rules_systemtest` closes the gap:
- **Caching.** A test's cache key is derived from both the test code and the description of
  the services it needs. Unchanged tests don't re-run.
- **Sharing.** A running fixture can be reused across many tests (subject to policy),
  amortizing startup cost. You choose the cost/parallelism trade-off per fixture.

Reusing fixtures across tests makes it easier to break hermeticity (one test can leave
state in the fixture that could affect subsequent tests) so the added risk and complexity
must be balanced against potential time savings.

`rules_systemtest` is particularly well suited for cases where test fixtures have to run
in an external environment (either because they can't fit on a single machine or because
the tests require running the service in a specific environment). In those cases fixture
startup tends to be long and the automatic fixture lifecycle management performed by the
coordinator allows for efficient management of shared resources.

If your dependencies are local processes that don't take a lot of time to start consider
[`rules_itest`](https://github.com/hermeticbuild/rules_itest) instead. It runs services as
child processes of the test action. There is no daemon and every test gets a fresh private
environment which gets cleaned up with the test action.

| | `rules_itest` | `rules_systemtest` |
|---|---|---|
| Where fixtures run | child processes of the test action | wherever the plugin provisions them: local container, K8s cluster, etc.. |
| Fixture lifetime | exactly one test action | independent of any test |
| Startup cost | paid per test action | amortized across tests |
| Sharing between tests | none; always hermetic | yes; per-fixture reuse policy |

The two are not exclusive. A repository can use both rulesets as appropriate.

---

# Concepts

- **Fixture** — one external service (or a small self-contained bundle of them) that a test
  depends on, e.g. "a Redis reachable at `$REDIS_MAIN_HOST:$REDIS_MAIN_PORT`". A fixture is
  produced by exactly one plugin call.
- **Fixture description** — the complete, content-addressed recipe for a fixture: which
  plugin builds it, and the inputs to that plugin. It fully determines the fixture's
  behavior; it is the basis of caching and reuse.
- **Plugin** — an out-of-process gRPC service that knows how to build a kind of fixture
  (a "Kubernetes deployment", a "local Docker container", a "database schema"). One plugin
  binary may implement several fixture kinds.
- **Coordinator** — the service the runner talks to. It decides whether to reuse an existing
  fixture or ask a plugin to build a new one, enforces quota, and hands leases back.
- **Lease** — a runner's claim on a fixture for the duration of a test. Multiple leases may
  share one fixture (per policy). A lease is kept alive by heartbeats and released at the end.
- **Runner** — the process Bazel actually executes for a test target. It obtains leases,
  runs hooks, executes the user's test binary, and reports results.

**Invariant (identity).** Two fixture descriptions that are byte-for-byte identical after
canonicalization denote the same fixture and are eligible to share one running instance. All
inputs that affect behavior — file contents, image digests, the plugin's own bytes — must be
part of the description, or caching and reuse are unsound.

---

# Architecture

```
   Bazel test action
   ┌───────────────────────────┐         ┌───────────────────────┐        ┌──────────────────┐
   │ test runner               │  gRPC   │ coordinator           │  gRPC  │ plugin           │
   │  ├ establish client id    ├────────►│  ├ reuse or provision ├───────►│  ├ GetResourcePool│
   │  ├ inline file inputs     │TakeLease│  ├ quota (pools)      │Create  │  ├ GetResource…   │
   │  ├ PreHooks (imageupload) │         │  ├ leases / sharing   │Fixture │  ├ CreateFixture  │
   │  ├ AroundHooks (portfwd)  │◄────────┤  └ persist state      │◄───────┤  └ …              │
   │  ├ run user test binary   │ outputs │                       │        │        │         │
   │  └ PostHooks (logs)       │         └───────────────────────┘        └────────┼─────────┘
   └───────────────────────────┘                                                  ▼
                                                                             external service
                                                                          (container, cluster, DB…)
```

The coordinator is **schema-agnostic**: it forwards a fixture's structured outputs to the
runner verbatim and never interprets them. Only plugins (which produce outputs) and the
runner's hooks (which consume them) understand output schemas.

---

# Starlark rules

## Fixture types and the fixture rule factory

A *fixture type* is defined once with a load-time factory that returns a Bazel `rule`. In
Bazel a rule can only be created while a `.bzl` file loads, so the factory is called at the
top level of a `.bzl` and bound to a global; the resulting rule is then instantiated as a
target in `BUILD` files.

```python
# in fixtures.bzl
load("@rules_systemtest//systemtest:defs.bzl", "systemtest_fixture_rule")

docker_run = systemtest_fixture_rule(
    impl_id = "@systemtest//fixtures:docker_run",
    inputs  = {
        "image":   "string",        # a digest-pinned image ref
        "command": "string_list",
        "ports":   "string_dict",   # {"main": "6379"}
    },
)
```

```python
# in BUILD.bazel
load(":fixtures.bzl", "docker_run")

docker_run(
    name   = "redis",
    image  = "registry.example.com/redis@sha256:...",
    ports  = {"main": "6379"},
    plugin = "//plugins:local",
    sharing = {"scope": "unrestricted", "max_connections": "10"},
)
```

**Input kinds** (`inputs` values):

| Kind | Meaning | On the wire |
|---|---|---|
| `string`, `int`, `bool` | scalar literal | `Value.str` |
| `string_list`, `string_dict` | structured literal | `Value.str` holding JSON |
| `file` | an artifact whose **contents** the runner inlines before contacting the coordinator | `Value.str` = runfiles path; key listed in `file_inputs` |
| `image` | a locally-built OCI image the `imageupload` hook pushes to a registry and replaces with an @sha256 ref | `Value.str` |

There is no notion of one fixture depending on another fixture's output; a fixture is a
single plugin call. (Composing fixtures out of smaller reusable steps is intentionally out of
scope — a plugin encapsulates whatever internal steps it needs.)

Every generated fixture rule has these attributes in addition to its declared inputs:

- `plugin` (label, required) — the `systemtest_plugin` that implements this `impl_id`.
- `sharing` (string dict) — `{"scope": "unrestricted"|"client_restricted"|"single_use",
  "max_connections": "<N>"}`. Defaults to `client_restricted`, `max_connections=1`.
- `startup_timeout_s` (int) — how long the runner/coordinator waits for the fixture to become
  healthy.

The rule produces a `SystemtestFixtureInfo` provider carrying the serialized fixture
description file and the runtime inputs (plugin binary, image files, file inputs) in its
runfiles.

## The `systemtest` test rule

`systemtest` is the `bazel test` entry point. It ties a set of fixtures to a test binary.

```python
systemtest(
    name     = "redis_test",
    test     = "//src/redistest:test",   # any *_test target: go_test, java_test, py_test…
    fixtures = [":redis"],
)
```

At analysis time it:

1. Collects each fixture's serialized description + runfiles.
2. Collects the inner test binary and its runfiles.
3. Resolves the runner binary from the runner toolchain.
4. Emits a small launcher script that invokes the runner with the fixture description paths,
   the inner test binary, and the coordinator connection config.

The launcher is the test executable Bazel runs. It sets `requires-external` (so Bazel does
not sandbox away network access) and a generous default timeout (fixture startup counts
against the test's wall clock).

`fixtures` may list several fixtures; the runner takes **one lease per fixture** and the test
process sees all fixtures' endpoints (see *Environment injection*).

## Plugins (`systemtest_plugin`)

A plugin bundles one or more fixture-kind implementations behind one binary/container,
multiplexed by `impl_id`.

```python
systemtest_plugin(
    name          = "local",
    binary        = "//plugins/local:plugin",   # local mode: an executable the coordinator starts
    container_ref = "registry.example.com/systemtest-plugin@sha256:...",  # remote mode
    impl_ids      = [
        "@systemtest//fixtures:docker_run",
        "@systemtest//fixtures:k8s",
    ],
)
```

At least one of `binary` (used in local mode) or `container_ref` (remote mode, must be
digest-pinned) is required. `impl_ids` lists the fixture kinds it implements; the fixture
rule checks its `impl_id` is in this list.

## The runner and coordinator toolchains

The runner and the coordinator are both resolved through toolchain types
(`@rules_systemtest//systemtest/toolchains:runner_toolchain_type`,
`:coordinator_toolchain_type`) rather than being hardcoded labels. That is what lets the
ruleset ship precompiled binaries by default while still allowing them to be replaced —
either by a user's own build, or by the from-source module (see *Distribution*).

Separate types per role on purpose: replacing the runner is common, replacing the
coordinator is rare, and one bundled type would force anyone doing either to re-specify
the other.

The runner is a normal binary composed of hooks (see *The test runner*). Users can supply
their own so they can add hooks that understand their plugins' custom output schemas.

```python
go_binary(
    name = "my_runner",
    srcs = ["main.go"],
    deps = ["@rules_systemtest_src//runner/pkg/runner", ...hooks...],
)
systemtest_runner(name = "my_systemtest_runner", binary = ":my_runner")
systemtest_runner_toolchain(name = "my_runner_toolchain", runner = ":my_systemtest_runner")
# in MODULE.bazel: register_toolchains("//tools:my_runner_toolchain")
```

A registered custom runner transparently replaces the built-in default: `register_toolchains`
in the root module outranks any registration a dependency makes. **Using non-standard plugins
means shipping a custom runner whose hooks understand those plugins' output schemas** — the
coupling between a plugin's output schemas and the runner's hooks is a compile-time concern;
there is no runtime schema-negotiation.

## Distribution

Two Bazel modules, released in lockstep (one BCR pull request publishes both):

- **`rules_systemtest`** — the Starlark rules, the proto and its `proto_library`, the
  toolchain types, and the coordinator / api gateway / test runner / `fixturectl` as
  *precompiled* per-platform binaries. Its only direct dependencies are `bazel_skylib`,
  `protobuf`, and `platforms`.
- **`rules_systemtest_src`** — the source for those binaries, and therefore `rules_go`,
  `gazelle`, `rules_rust`, `rules_rust_prost`, and `crate_universe`.

The split is the reason the `proto_library` lives in the ruleset while
`go_proto_library` / `rust_prost_library` live in the source module: a consumer of the
released ruleset must never *use* a language ruleset, but a plugin author in any language
still needs the `proto_library` to generate their own stubs. (`proto_library` requires its
`srcs` in the same package, so exporting only the raw `.proto` would not work — plugin
authors could not declare their own.)

What the split buys is the elimination of extension evaluation and toolchain registration:
no `go_deps` resolution against the consumer's Go module graph, no `crate_universe` repin,
no Rust or Go toolchain download. It does not empty the module *graph* — `protobuf`
transitively names `rules_go`, `gazelle`, and `rules_rust` — but nothing loads from them,
so their repos are never fetched.

Building from source is opt-in, in the consumer's own MODULE.bazel:

```python
bazel_dep(name = "rules_systemtest_src", version = "<rules_systemtest version>")
register_toolchains("@rules_systemtest_src//toolchains:all")
```

Locally the coordinator and the api gateway run as one process; remotely they are two
services. Both are composed from the same libraries, so the difference is which binary
is built, not conditional compilation.

## High-level macros

Most users use macros that hide the fixture-type definition:

```python
docker_fixture(name = "redis", image = "registry.example.com/redis@sha256:...",
               ports = {"main": "6379"})

k8s_fixture(name = "db", kubeconfig = "//infra:kubeconfig", manifest = "db.yaml",
            image = ":ephemeral_image")
```

Each macro instantiates a built-in fixture type with the built-in plugin.

## Configuration

Two repository-level settings (Bazel flags, or a small module extension):

- `mode` = `local` (default) or `remote`.
- `remote_address` — the coordinator URL, in remote mode.

These flow to the runner via the launcher script.

## Cacheability invariants

`systemtest` targets are cached by Bazel like any test. For that to be correct:

- **The fixture description must fully determine the fixture's behavior.** Anything that
  affects the running service must be an input to the description.
- **Image refs must be digest-pinned.** A mutable tag makes the description non-hermetic.
- **Ephemeral (locally-built) images** are pushed by the runner and the input is rewritten to
  the resulting digest before the description reaches the coordinator; the image artifact is
  in the test action's runfiles, so rebuilding it re-runs the test.
- **File inputs are carried by content**, not path (see *File-content inlining*), and the
  files are in the test action's runfiles.
- **The plugin's own bytes are part of fixture identity** (the coordinator hashes the plugin
  binary / uses the digest-pinned container ref). A plugin change therefore both re-runs the
  Bazel test (the plugin binary is in runfiles) and invalidates coordinator-side reuse.

---

# The test runner

The runner is language-agnostic: it wraps the inner test binary as a child process, so it
works with `go_test`, `java_test`, `py_test`, etc.

## Lifecycle

For each `systemtest` invocation the runner:

1. **Establishes client identity** (below).
2. For each fixture: loads its description, runs **PreHooks** (which may mutate the
   description — e.g. `imageupload`), **inlines file-input contents**, connects to the
   coordinator, and calls `TakeLease`.
3. **Waits for readiness.** If the fixture is being built, `TakeLease` returns `PENDING`; the
   runner polls `Status` until `SUCCEEDED` (or `FAILED`), sending `Keepalive` heartbeats
   throughout. Startup time counts against the test timeout.
4. **Sets up AroundHooks** — each returns a wrapping around the test `*exec.Cmd`
   (e.g. `portforward` binds local ports and injects env vars). Wrappings compose
   decorator-style.
5. **Injects environment** from every lease's outputs (below) and runs the inner test binary.
6. **Runs PostHooks** — e.g. `k8slogs`/`locallogs` collect fixture logs.
7. **Releases every lease**, writes a JUnit `test.xml`, and exits with the test's code.

## Client identity

`client_id` is the reuse key: a deterministic hash of the workspace path + username +
the machine's node id, plus any operator-supplied extra (for CI, e.g. a change id or
author). Two runs with the same `client_id` and the same fixture may share a running
fixture (subject to sharing scope). It must be stable across runs for reuse to work and
distinct between users/checkouts for isolation.

## File-content inlining

A `file` input reaches the runner as a runfiles path (`Value.str`) whose key is listed in
`FixtureDescription.file_inputs`. Before contacting the coordinator, the runner replaces each
such value with the file's **contents** (`Value.data`). Consequences:

- The coordinator and plugin only ever see contents, so a remote coordinator works without
  filesystem access to the client.
- File contents therefore participate in fixture identity (the coordinator hashes the
  content-resolved description).

## Hook interfaces

Hooks are the extension surface. All three kinds are ordinary Go interfaces.

```go
package runner

// PreHook runs before TakeLease and may mutate the fixture description
// (e.g. push images and rewrite refs). Runs in registration order.
type PreHook interface {
    Name() string
    BeforeLease(HookCtx, *FixtureDescription) error
}

// AroundHook brackets test execution. Setup returns a Wrapping that decorates
// the test command; Cleanups run in reverse (defer-style).
type AroundHook interface {
    Name() string
    Setup(HookCtx, *Lease) (Wrapping, error)
}

type Wrapping struct {
    Wrap    func(*exec.Cmd) *exec.Cmd  // decorate env/args/stdio
    Cleanup func() error               // non-nil error fails the test
}

// PostHook runs after the test exits, before ReleaseLease.
type PostHook interface {
    Name() string
    AfterTest(HookCtx, *Lease, TestResult) error
}

type HookCtx struct {
    Context context.Context
    Args    RunArgs   // parsed CLI args + connection config
    Log     Logger
}

// A resolved fixture the hooks operate on. Outputs are schema-tagged Values
// forwarded verbatim from the plugin (the coordinator does not interpret them).
type Lease struct {
    LeaseID     string
    FixtureName string             // the fixture target name (env-var prefix)
    Outputs     map[string]*Value  // by output name; each Value is typically a SchemaBlob
}

type TestResult struct {
    ExitCode int
    Duration time.Duration
}

type Runner struct { /* … */ }

func New() *Runner
func (r *Runner) WithPreHook(PreHook) *Runner
func (r *Runner) WithAroundHook(AroundHook) *Runner
func (r *Runner) WithPostHook(PostHook) *Runner
func (r *Runner) Run(ctx context.Context, args RunArgs) int  // returns process exit code
```

Hooks decide whether to fire by **inspecting the lease's outputs for a schema id they
understand** (e.g. `portforward` fires only if an output carries the `systemtest.ports`
schema). They never key on `impl_id`, which is an arbitrary user label.

## Environment injection

For each lease, the runner injects environment variables the inner test reads. Given a
fixture target named `redis` exposing a port named `main`, the test sees:

```
REDIS_MAIN_HOST=127.0.0.1
REDIS_MAIN_PORT=54231
```

The prefix is the uppercased, sanitized fixture name; this keeps multiple fixtures in one
test distinct. Ports come from the `systemtest.ports` output (below); the `portforward` hook
substitutes the locally-bound host/port for tunneled ports.

## Stock hooks

| Hook | Type | Fires when | Does |
|---|---|---|---|
| `imageupload` | Pre | any `image_inputs` present | pushes each local image to the configured registry by digest (HEAD then PUT if absent), rewrites the input to the resolved digest ref |
| `portforward` | Around | an output has schema `systemtest.ports` with a tunneled entry | binds a local port per tunneled entry (via `kubectl port-forward`), injects host/port env; cleanup tears the forwards down |
| `k8slogs` | Post | an output has schema `systemtest.k8s_log_fetch` | materializes a temp kubeconfig from the descriptor's contents, `kubectl logs` per pod (bounded, partial-OK), writes to the test's undeclared-outputs dir |
| `locallogs` | Post | an output has schema `systemtest.local_log_fetch` | reads the listed local files and writes them to the undeclared-outputs dir |

## Building a custom runner

```go
func main() {
    os.Exit(runner.New().
        WithPreHook(imageupload.FromEnv()).       // SYSTEMTEST_REGISTRY=…; no-op if unset
        WithAroundHook(portforward.Default()).
        WithPostHook(k8slogs.Default()).
        WithPostHook(locallogs.Default()).
        // your hooks, understanding your plugins' schemas:
        WithPostHook(mylogs.Default()).
        Run(context.Background(), runner.ParseArgs(os.Args)))
}
```

The hook framework is an ordinary Go library — `github.com/…/rules_systemtest/src/runner/pkg/runner`
— so a custom runner is built with the user's own `rules_go`. The ruleset ships the default
runner precompiled and does not require `rules_go` of anyone who uses it as-is.

---

# The coordinator

## Modes

- **Local** (default). A single-process daemon managing fixtures for one workspace on one
  machine. It listens on a Unix Domain Socket at a stable per-workspace path (e.g.
  `/tmp/systemtest/<hash-of-workspace-path>/server.sock`). State is in-memory. There is **no
  authentication** — the socket is used only by the local user's own test processes. The
  daemon is started lazily by the first runner that finds no socket (use a file lock at the
  socket path to avoid a start race between concurrent test actions).
- **Remote.** A hosted, multi-user, multi-workspace service reached over a URL. State is in a
  PostgreSQL database. Authenticated with a bearer token (below).

The two modes share the same API and semantics; they differ only in transport, storage, and
auth. (The SQL for the remote store and any local persistent store can diverge substantially;
keeping local mode in-memory sidesteps that.)

## Runner ↔ coordinator API

```proto
service CoordinatorService {
  rpc TakeLease(TakeLeaseRequest) returns (TakeLeaseResponse);
  rpc Keepalive(KeepaliveRequest) returns (KeepaliveResponse);
  rpc ReleaseLease(ReleaseRequest) returns (ReleaseResponse);
  rpc Status(StatusRequest) returns (StatusResponse);
}

message TakeLeaseRequest {
  FixtureDescription fixture = 1;   // fully resolved: files inlined, images digest-pinned
  string client_id = 2;             // reuse key
  string owner = 3;                 // accounting only
  google.protobuf.Duration ttl = 4; // lease is GC'd if not kept alive within ttl
  uint32 associated_tests_count = 5;
}

message TakeLeaseResponse {
  LeaseStatus status = 1;                 // PENDING | SUCCEEDED | FAILED
  string lease_id = 2;
  map<string, Value> outputs = 3;         // only on SUCCEEDED; forwarded verbatim from the plugin
  repeated string messages = 4;           // human-readable progress/errors
}

message KeepaliveRequest  { string lease_id = 1; }
message KeepaliveResponse {}
message ReleaseRequest    { string lease_id = 1; }
message ReleaseResponse   {}
message StatusRequest     { string lease_id = 1; }
message StatusResponse {
  LeaseStatus status = 1;
  map<string, Value> outputs = 2;
  repeated string messages = 3;
}

enum LeaseStatus { LEASE_STATUS_UNKNOWN = 0; PENDING = 1; SUCCEEDED = 2; FAILED = 3; }
```

## Authentication

We will make it possible to configure a remote authentication service in addition to an allow all/deny all setting.
Headers returned by the remote authentication service will be attached to client requests and forwarded to the plugins.

## Reuse, sharing, and slots

Each fixture carries a `Sharing` policy:

```proto
message Sharing {
  message Empty {}
  oneof use_scope {
    Empty unrestricted     = 1;  // any client_id may reuse
    Empty client_restricted = 2; // only the client_id that created it (the default)
    Empty single_use       = 3;  // never reused
  }
  int64 maximum_concurrent_connections = 4;  // "slots": how many leases may share it at once
}
```

**Reuse rule.** A `TakeLease` may attach to a live fixture iff:

1. its **fingerprint is identical** (always required), and
2. the fixture's `use_scope` admits the caller: `unrestricted` → any `client_id`;
   `client_restricted` → the same `client_id`; `single_use` → never, and
3. a **slot is free** (`used_slots < maximum_concurrent_connections`).

Otherwise the coordinator provisions a new fixture. `client_restricted` is the default, so by
default a fixture is private to the user that created it; `unrestricted` is the explicit
opt-in to cross-user sharing.

Attaching a lease takes a slot; releasing returns it. When a fixture has no leases it becomes
idle and is eligible for garbage collection after its `idle_timeout` (reported by the plugin).

## Fixture identity (fingerprint)

**The coordinator computes the fingerprint — the runner never does, and any client-supplied
value is ignored.** (Reuse correctness must not depend on every client canonicalizing
identically.) On receiving a `TakeLeaseRequest`, the coordinator:

1. Takes the (already content-resolved) description: file contents inlined, image inputs
   digest-pinned.
2. Derives the **plugin content hash**: in local mode, a hash of the plugin binary's bytes;
   in remote mode, the `sha256:`-pinned `container_ref`.
3. Canonicalizes the description deterministically (sorted map keys, fixed field order,
   canonical scalar encoding) and hashes it together with the plugin content hash.

The resulting digest is the fingerprint. **Invariant:** identical resolved descriptions +
identical plugin bytes ⇒ identical fingerprint ⇒ reuse; any behavioral change ⇒ different
fingerprint ⇒ a fresh fixture.

## The allocator

All quota and fixture-store mutations run on a **single logical driver** so bookkeeping stays
consistent. This does *not* serialize provisioning — the actual `CreateFixture` calls run
concurrently.

```
on TakeLease(desc, client_id):
    fp = fingerprint(desc)                       # coordinator-computed (above)
    lock:
        if f := find_reusable(fp, client_id): 
            f.used_slots += 1
            return SUCCEEDED(lease→f, f.outputs)
        pool = plugin.GetResourcePool(desc)      # register idempotently (first-wins, below)
        need = plugin.GetResourceRequests(desc)  # estimate, >= actual
        if not pool.can_afford(need):
            gc_idle_fixtures()                    # try to free capacity
            if still not affordable:
                enqueue(request); return PENDING  # retried when capacity frees
        pool.charge(need)                         # reserve
        record = new_provisioning_record(fp, client_id, need)
    # outside the lock — concurrent:
    dispatch:
        stream = plugin.CreateFixture(desc, metadata{fixture_id=record.id, client_id, owner})
        on success(result): record.outputs = result.outputs; record.ready = true
        on error(e):        record.failed = e; pool.refund(need); cache_error(fp, e)
    return PENDING          # runner polls Status; on ready → SUCCEEDED + outputs
```

Notes:

- **Error caching.** A fixture that fails to build has its error cached against its
  fingerprint for an **exponentially increasing** window, so transient failures retry soon
  but a persistently-broken fixture doesn't burn resources on every test.
- **Backpressure.** When quota is exhausted and no idle fixture can be freed, requests stay
  `PENDING` (queued) until capacity or a reusable fixture appears.

## Resource pools (quota)

Each plugin declares the **single pool it operates within** for a given description, measured
at runtime, via `GetResourcePool`:

```proto
message ResourceAmount { string name = 1; string unit = 2; int64 quantity = 3; }  // e.g. cpu/milli
message ResourcePool   { string pool_id = 1; repeated ResourceAmount resources = 2; }
```

- A pool holds several resource kinds at once (e.g. cpu + memory + pods).
- `pool_id` is derived by the plugin from the description (e.g. a cluster host, or `"local"`)
  so concurrent fixtures over the *same* infrastructure name the *same* pool.
- The coordinator **registers pools idempotently by `pool_id`, first-wins**: the first
  measurement of a pool's capacity is authoritative; a later, conflicting measurement is
  ignored with a warning (this avoids double-counting and needs no operator config, at the
  cost of not tracking capacity that changes while fixtures are live).

Consumption is **estimate-only**:

- `GetResourceRequests(desc)` returns an estimate of what the fixture will consume; it **must
  be ≥ the actual consumption**.
- The coordinator charges the estimate on create and refunds exactly it on destroy. There is
  no reconcile step. An over-generous estimate wastes capacity; that is accepted. (Quota is
  therefore advisory at the infrastructure boundary — a plugin that under-estimates can
  over-commit real infrastructure. A plugin may optionally report actual `consumed` for a
  best-effort "consumed ≤ estimate" assertion.)

## Lost-fixture recovery

A fixture may be "lost" if the coordinator crashes after the plugin created it but before the
record is persisted. To recover: the coordinator passes a stable `fixture_id` (a UUID) in
`CreationMetadata`, and plugins **must make `CreateFixture` idempotent keyed on it** (e.g. use
it as the created resource's name/label). A retried create then re-attaches to the existing
resource instead of creating a second one.

The stored fixture record must also carry enough to *re-reach* the plugin later for
`DestroyFixture`/`HealthCheck` (the plugin reference / content hash), not just the plugin's
opaque `instance_data`.

## Storage

- **Local:** in-memory. Fixture records, leases, and pools live in the daemon's process
  memory; they vanish when it exits (which is fine — the fixtures it owns are typically
  ephemeral and re-provisioned on demand).
- **Remote:** PostgreSQL. Persist fixture records (description, fingerprint, `client_id`,
  outputs, `instance_data`, charged consumption, sharing, `idle_timeout`), leases (with
  last-keepalive timestamps for GC), and pool balances.

## Auth

- **Local:** none. The UDS is only reachable by the local user's processes.
- **Remote:** a bearer token, resolved in order: `SYSTEMTEST_AUTH_TOKEN` env var; a configured
  helper command that prints a token to stdout; otherwise anonymous (refused unless the server
  permits it).
- **Plugin ↔ external systems** (a cluster, a secret store, a registry): the plugin's own
  responsibility; the toolchain lets operators swap plugin implementations.

---

# The plugin protocol

## Coordinator ↔ plugin API

A plugin is a gRPC server. In local mode the coordinator starts the plugin binary and hands
it a socket to listen on; in remote mode the plugin runs as a (digest-pinned) container the
service can launch. One plugin multiplexes several fixture kinds by `impl_id`.

```proto
service CoordinatorPlugin {
  rpc DescribePlugin(google.protobuf.Empty) returns (DescribeResponses);
  rpc GetResourcePool(CreateRequest)  returns (ResourcePool);       // the pool it operates in
  rpc GetResourceRequests(CreateRequest) returns (ResourceRequests);// consumption estimate (>= actual)
  rpc CreateFixture(CreateRequest)    returns (stream CreateProgress);
  rpc DestroyFixture(DestroyRequest)  returns (DestroyResponse);
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}

message DescribeResponses { repeated DescribeResponse responses = 1; }
message DescribeResponse {
  string impl_id = 1;
  string version = 2;
  map<string, string> schemas = 3;  // schema_id -> JSON Schema text for the outputs it emits
}

message CreationMetadata {
  string fixture_id = 1;            // stable id; key idempotency on this
  string client_id  = 2;
  string owner      = 3;
  map<string, string> labels = 4;
}

message CreateRequest {
  string impl_id = 1;
  map<string, Value> inputs = 2;    // the fixture's parameters (file contents already inlined)
  CreationMetadata metadata = 3;
}

message ResourceRequests { repeated ResourcePool consumes = 1; }

enum CreateProgressStatus {
  CREATE_PROGRESS_UNKNOWN   = 0;
  CREATE_PROGRESS_ENQUEUED  = 1;
  CREATE_PROGRESS_CREATING  = 2;
  CREATE_PROGRESS_RUNNING   = 3;
  CREATE_PROGRESS_SUCCEEDED = 4;   // terminal success; `result` is set on this message
  CREATE_PROGRESS_ERROR     = 5;   // terminal failure
}

enum FixtureElementStatus {
  FIXTURE_ELEMENT_UNKNOWN  = 0;
  FIXTURE_ELEMENT_CREATING = 1;
  FIXTURE_ELEMENT_RUNNING  = 2;
  FIXTURE_ELEMENT_ERROR    = 3;
}
message FixtureElementStatusInfo {  // progress of a sub-component, e.g. one pod
  string element_name = 1;
  FixtureElementStatus status = 2;
  repeated string messages = 3;
}

message CreateProgress {
  CreateProgressStatus status = 1;
  repeated FixtureElementStatusInfo element_infos = 2;
  optional FixtureResult result = 3;  // set exactly once, on the CREATE_PROGRESS_SUCCEEDED message
}

message FixtureResult {
  map<string, Value> outputs = 1;         // all structured data for the runner's hooks
  google.protobuf.Duration idle_timeout = 2; // how long to keep it unused before GC
  bytes instance_data = 3;                 // opaque bookkeeping; handed back to Destroy/HealthCheck
  repeated ResourcePool consumed = 4;      // optional, validation-only vs the estimate
}

message DestroyRequest  { string impl_id = 1; bytes instance_data = 2; }
message DestroyResponse {}
message HealthCheckRequest  { string impl_id = 1; bytes instance_data = 2; }
message HealthCheckResponse { bool healthy = 1; string message = 2; }
```

## Idempotency & lifecycle

- A plugin process is kept alive as long as fixtures reference it; it must clean up on
  `SIGTERM` (send `SIGTERM`, wait, then `SIGKILL` — a bare kill skips the plugin's cleanup).
- `CreateFixture` **must be idempotent** on `CreationMetadata.fixture_id` so a retried create
  re-attaches rather than duplicating.
- `CreateFixture` streams progress and ends with exactly one `CREATE_PROGRESS_SUCCEEDED`
  message carrying the `FixtureResult`, or a `CREATE_PROGRESS_ERROR`.

## Plugin SDK

A thin library removes the gRPC boilerplate. The author implements an interface per fixture
kind and registers them; the SDK multiplexes on `impl_id`, aggregates `DescribePlugin`, and
handles the listen address + signals.

```go
type CoordinatorPluginImpl interface {
    Describe(ctx) (Description, error)                      // impl_id, version, schemas
    GetResourcePool(ctx, CreateRequest) (ResourcePool, error)
    GetResourceRequests(ctx, CreateRequest) (ResourceRequests, error)
    CreateFixture(ctx, CreateRequest, ProgressReporter) (FixtureResult, error)
    DestroyFixture(ctx, DestroyRequest) error
    HealthCheck(ctx, HealthCheckRequest) (HealthCheckResponse, error)
}

type ProgressReporter interface {
    Element(name string, status FixtureElementStatus, msg string)
    // The SDK emits CREATE_PROGRESS_SUCCEEDED with the returned FixtureResult.
}

func Serve(impls ...CoordinatorPluginImpl)  // reads the socket/listen address from flags/env; serves; handles SIGTERM
```

---

# Structured outputs and schemas

A fixture communicates everything the runner needs — endpoints, log-fetch instructions, a
kubeconfig, anything — through its `FixtureResult.outputs`, a `map<string, Value>`.

## The `Value` type

```proto
message Value {
  message SchemaBlob { string schema_id = 1; bytes contents = 2; }  // JSON conforming to schema_id
  oneof value {
    string str = 1;         // a scalar (also used for JSON-encoded lists/dicts on inputs)
    bytes  data = 3;        // raw bytes (e.g. inlined file contents on inputs)
    SchemaBlob schema_blob = 4;
  }
}
```

Structured outputs are `SchemaBlob`s: JSON tagged with the id of a schema the producing
plugin advertises. This is the single mechanism by which plugins hand data to hooks, and by
which hooks self-gate (a hook fires iff an output carries a schema id it understands).

## Schema ownership

**Schemas are owned by the plugin that emits them**, and advertised in
`DescribeResponse.schemas` (as JSON Schema text). The runner may validate a blob against the
advertised schema. Because a hook must understand a schema to consume it, a plugin's schemas
and the runner's hooks are coupled at build time: shipping a plugin with custom outputs means
shipping a runner with matching hooks. There is no central schema registry.

## Core schemas

The project ships a small vocabulary that the built-in hooks understand.

- **`systemtest.ports`** — connectable endpoints. Consumed by `portforward` + env injection.
  ```json
  { "ports": [
      { "name": "main", "protocol": "tcp",
        "tunnel": false, "host": "10.0.0.5", "port": 6379 },
      { "name": "admin", "protocol": "tcp",
        "tunnel": true, "namespace": "ns-abc", "target": "svc/redis", "remote_port": 6380 }
  ] }
  ```
  A `tunnel: false` entry is directly reachable at `host:port`. A `tunnel: true` entry must be
  reached via `kubectl port-forward` (see *Networking*); it names the cluster target and
  remote port, and pairs with a `systemtest.kubeconfig` output for cluster access.
- **`systemtest.kubeconfig`** — `{ "contents": "<kubeconfig>" }`. Cluster access for tunneling
  and/or for tests that talk to the cluster directly. Carried as contents; hooks materialize a
  temp file.
- **`systemtest.k8s_log_fetch`** — `{ "kubeconfig": "<contents, log-read-only>",
  "namespace": "…" }`. Consumed by `k8slogs`.
- **`systemtest.local_log_fetch`** — `{ "paths": ["/var/log/…"] }`. Consumed by `locallogs`.

---

# Networking

In most cases, we expect that a cluster is able to expose fixtures via an Ingress and that
tests will be able to use those addresses to directly access the fixture.

The only built-in way to reach a fixture that is not directly reachable is `kubectl
port-forward`, implemented by the `portforward` AroundHook. For each `systemtest.ports`
entry with `tunnel: true`, the hook:

1. materializes the `systemtest.kubeconfig` contents to a temp file,
2. runs `kubectl --kubeconfig <tmp> -n <namespace> port-forward <target> :<remote_port>`,
3. parses the locally-bound port, and injects `<PREFIX>_<NAME>_HOST=127.0.0.1` /
   `<PREFIX>_<NAME>_PORT=<local>` into the test's environment,
4. tears the forwards down in cleanup.

`tunnel: false` entries are injected directly as `host:port`. This assumes the fixture's
cluster is reachable from where the test runs (directly or via the kube API server). Reaching
services across a boundary that only exposes the coordinator is out of scope; an operator who
needs it writes an alternative AroundHook and ships it in a custom runner.

---

# Logs

Logs are fetched **client-side by hooks** using a descriptor the plugin returns in `outputs`
(the coordinator is not in the log path):

- `k8slogs` (Post): on a `systemtest.k8s_log_fetch` output, materializes the log-read-only
  kubeconfig and runs `kubectl logs` for the selected pods, in parallel, with a time cap
  (partial results are acceptable). Output goes to the test's undeclared-outputs directory.
- `locallogs` (Post): on a `systemtest.local_log_fetch` output, copies the listed files.

A plugin that stores logs elsewhere defines its own log-fetch schema and ships a matching
PostHook.

---

# Image upload & registries

Locally-built (ephemeral) images are pushed by the `imageupload` PreHook, not by the
coordinator. For each `image_inputs` key:

1. read the local OCI image (an OCI layout the Bazel image rule produced),
2. `HEAD` the target registry by digest; if absent, `PUT` the blobs + manifest,
3. rewrite the input `Value` to the resulting digest-pinned ref, so the description that
   reaches the coordinator is hermetic.

The registry endpoint and auth come from configuration (env/flag); any OCI-compliant registry
works. Digest-pinned image inputs (not built locally) need no upload and are passed through.

---

# Debugging

A small CLI (`fixturectl`) aids debugging against a coordinator:

- list the caller's fixtures, and their status/outputs,
- clear (destroy) the caller's fixtures of a given kind,
- provision a single fixture from a description and print its outputs (endpoints, kubeconfig,
  …) without running a test.

---

# Appendix: common messages

```proto
message FixtureDescription {
  string impl_id = 1;
  oneof plugin {
    string binary_path   = 2;   // local mode: the coordinator starts this
    string container_ref = 3;   // remote mode: digest-pinned
  }
  map<string, Value> inputs = 4;      // literals; inlined file contents; digest-pinned image refs
  repeated string image_inputs = 5;   // input keys the imageupload hook resolves
  repeated string file_inputs  = 6;   // input keys the runner inlines as file contents
  Sharing sharing = 7;
  google.protobuf.Duration startup_timeout = 8;
  string name = 9;                    // human-readable; not part of identity
  // Note: no fingerprint field — identity is computed by the coordinator.
}
```
