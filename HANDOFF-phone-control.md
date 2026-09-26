# Phone control yields to desktop shell activity

2026-09-26. Worktree `/Users/shuv/repos/herdr-phone-handoff`, branch
`phone-control-handoff`, based on `v0.9.1-shuv.3` / `63544697443400dd1b3d5e81710300fff6e0495c`.
Fork remote: `Latitudes-Dev/herdr`. No push, tag, release, installation, production
server restart, or production live handoff was performed.

## Design

Ownership is a server runtime fact. Reuse the existing direct-controller takeover
shutdown and removal path: send `terminal attach taken over`, remove the direct
client and resize lock, restore the previous shell geometry controller, then
apply the interacting shell's geometry claim. No new wire message or client-side
timeout is needed. SSHuv receives the existing `terminal.closed` record and can
open a fresh `control --takeover` stream later.

Triggers are active ClientShell focus gained (including a repeated true value),
successful explicit pane/directional/agent/tab/workspace selection, navigation
that changes the selected pane, accepted pane input under the existing interaction
classification, and image-path paste. Input reclaims its actual target pane,
which can differ from the focused pane in a split tab. Rejected/stale/inactive
input does not reclaim. Key/mouse release-only batches do not count as interaction.
Rejected focus requests preserve direct control.

Passive shell resizes, rendering, output, observers, focus loss, and background
geometry reconciliation do not reclaim control. Resizes often arrive without
user intent, so using them would let a passive desktop steal control immediately
after the phone acquires it. An explicit focus/input is the arbitration signal.
Only the selected/input terminal is reclaimed; another tab's controller stays.
Public JSON API writers retain their existing behavior: this change is scoped to
ClientShell activity, including ClientShell connections through remote bridges.

The owner-removal helper is shared with direct-to-direct takeover. Observers
never enter that map and stay connected. Live-handoff state/protocol and remote
bridge transports are unchanged. The reclamation guard declines during live
handoff. Protocol 22 and endpoint generation 1 remain unchanged; frozen codecs
are untouched. The capability is additive JSON with a false deserialization
default for old servers, and an optional endpoint capability string.

Work is on explicit input/navigation events, with an empty-owner fast path. No
render/layout/fanout loop or pane-scaled background work was widened; render-scale
benchmarking is therefore not applicable to this change.

## Changed files

Line references refer to this worktree's committed contents.

- `src/server/headless.rs:1113`: shared takeover helper and shell target guards;
  `:1349` image paste; `:1944` existing direct takeover uses the helper;
  `:2419` explicit outer focus; `:2566` pane input.
- `src/server/headless/client_views.rs:915`: successful endpoint focus/navigation
  reclaims control, including same-pane selection; rejected focus does not.
- `src/api/schema/server.rs:23`, `src/api/server.rs:70`: typed capability and
  server advertisement; `src/api/server.rs:1304` tests live ping serialization
  and absent-field compatibility.
- `src/cli/status.rs:279`: server status JSON capability projection; `:303`
  installed-client endpoint capability advertisement; `:441` status test.
- `src/protocol/endpoint.rs:27`: endpoint capability string, welcome advertisement,
  and compatible-server capability test.
- `src/server/headless/tests/direct_control.rs:1`: fixture with shell, controller,
  observer, and real terminal-core geometry. `:81` tests focus, repeated focus,
  pane/tab/workspace selection and text input; it also tests reacquisition,
  subsequent direct takeover, and late old-client disconnect. `:225` checks
  passive/rejected/inactive/observer actions. `:319` checks another tab. `:347`
  tests image paste. `src/server/headless/tests/mod.rs:3` registers the module.
- `src/api/schema/tests.rs:723`, `src/update.rs:2949`: capability literals in
  existing fixtures, preserving their previous semantics.
- `docs/next/api/herdr-api.schema.json:10663`: regenerated optional JSON field.
- `docs/next/website/src/content/docs/cli-reference.mdx:382` and corresponding
  Japanese/Chinese references at `:357`: documented arbitration and detection.
- `CHANGELOG.md:5`: fork shuv.4 entry. Cargo base remains `0.9.1`, just as in all
  three prior fork tags. Build with `HERDR_FORK_REVISION=4`; putting the suffix
  into Cargo.toml would diverge from the fork release workflow's version scheme.

## Binary

On `shuvdev`:

```text
/home/shuv/src-scratch/herdr-shuv4/herdr-linux-x86_64-0.9.1-shuv.4
SHA-256: 69d84185b186d2e274673a452bc51a5ed3410c130f78914a64f2d50a91dd9116
Version: herdr 0.9.1-shuv.4+63544697-shuv4-worktree
```

This is a statically linked Linux x86_64 musl release binary, using the existing
fork workflow's target and Ghostty optimization settings. `shuvdev` actually
reported Omarchy/Arch, rather than Ubuntu. The musl artifact avoids dependence
on its glibc version. The metadata suffix identifies a build of the patched
worktree based on 63544697, not a claim that the old commit contains the fix.
The complete source snapshot and build/test logs are beside the binary. The
original `~/.local/bin/herdr` was not changed.

Build command in the remote scratch directory (Zig 0.16.0):

```sh
HERDR_FORK_REVISION=4 HERDR_BUILD_COMMIT=63544697-shuv4-worktree \
LIBGHOSTTY_VT_OPTIMIZE=ReleaseFast LIBGHOSTTY_VT_SIMD=true CARGO_BUILD_JOBS=6 \
~/.cargo/bin/rustup run stable cargo build --release --locked \
  --target x86_64-unknown-linux-musl
```

## Validation

- Mac `HERDR_FORK_REVISION=4 ZIG=/Users/shuv/.local/opt/zig-aarch64-macos-0.16.0/zig CARGO_BUILD_JOBS=4 cargo build --locked`: **passed**, 1m30s, log `/tmp/herdr-shuv4-build.log`.
- Linux release build above: **passed**, 1m40s. Existing warning:
  deprecated musl `libc::time_t` at `src/platform/unix_common.rs:400`.
- Linux `cargo test --release --locked --target x86_64-unknown-linux-musl direct_control -- --test-threads=1` with the same build environment: **5 passed, 0 failed** (four server regression tests, including the six-trigger loop, and one capability test). Log `test-direct-control.log`.
- Linux `cargo +stable nextest run --release --locked --target x86_64-unknown-linux-musl -E 'test(server::) | test(api::) | test(protocol::) | test(cli::status::) | test(client::handshake::)' --status-level fail --final-status-level fail --failure-output final --success-output never`: **730 passed, 0 failed, 3061 skipped**. Log `test-nextest-linux.log`. Includes frozen endpoint contracts, remote bridges, existing takeover and disconnect behavior.
- Full release-mode nextest with `--no-fail-fast` and no filter: **3768 passed,
  15 failed, 8 skipped**. Log `test-nextest-full-linux.log`. Failures include
  unsuffixed-version expectations, release-note identity, and integration tests
  that expect `herdr-dev` paths although release builds use `herdr` paths.
  These do not recur in the normal debug run below.
- Linux normal debug configuration: `PATH="$HOME/.cargo/bin:$PATH" CARGO_BUILD_JOBS=6 cargo +stable nextest run --locked --no-fail-fast --status-level fail --final-status-level fail --failure-output final --success-output never`: **3783 passed, 0 failed, 8 skipped** across 16 binaries (13.501s after a 1m20s build). No fork/build-identity overrides were set for this run, matching the repository's normal test configuration. Includes all live-handoff integration tests. Log `test-nextest-debug-linux.log`. The actual suffixed release binary was separately validated by the 730-test run and final live smoke.
- Initial full release `cargo test` runs (parallel and `--test-threads=1`) ended
  with **SIGPIPE, exit 101**, so they are not passing runs. The serial run stopped
  at `client_shell_attach_seeds_workspace`; both reported cascading failures
  after earlier assertions. Logs `test-linux.log`, `test-linux-serial.log`.
- Those runs exposed the missing generated API schema update; it was regenerated
  with `HERDR_UPDATE_API_SCHEMA=1` using the schema test and then verified by the
  passing 730-test run. An initial test compilation also caught incorrect test
  registry indexing, corrected to use the registry's `get()` accessor.
- `just maintenance-test`: **158 Python tests and 5 Bun tests passed**.
  `just ui-hot-path-architecture-test`: **6 passed**.
  Logs `/tmp/herdr-shuv4-maintenance.log`.
- `just docs-contract-test`: **7 passed**, including a final run after localized
  documentation updates. Log `/tmp/herdr-shuv4-docs-final.log`.
- `cargo fmt --check` and `git diff --check`: **passed**.
- No complete Mac test result: other SSHuv Xcode jobs repeatedly occupied the
  host. An initial Rust test launch overlapped a newly started Xcode job briefly
  and was stopped; subsequent guarded attempts declined to start. Heavy Rust
  testing was moved to shuvdev. No Xcode process was stopped.
- `just check` as a whole was not run; the requested build/tests plus the above
  maintenance/docs checks were used. Windows cross-lint was not run.

## Live verification and cleanup

The final delivered binary was tested via `smoke.py`, with output in
`/home/shuv/src-scratch/herdr-shuv4/smoke-final.log`.

A separate headless `--session sshuv-handoff-test` ran under isolated
`XDG_CONFIG_HOME`, `XDG_STATE_HOME`, and `HERDR_CONFIG_PATH` in
`/var/tmp/herdr-shuv4-7zrnjz2t`. All inherited Herdr socket/session/caller overrides
were cleared. A real ClientShell TUI ran under a 120x40 pseudoterminal; a scratch
child polled `stty size` to report the actual PTY dimensions independently of
observer frame cropping. No user session or paid agent was used.

Observed with the final artifact:

1. Server status reported `direct_control_yields_to_shell: true`.
2. Desktop PTY was **93 columns x 39 rows**.
3. `terminal session control <discovered-pane> --takeover --cols 50 --rows 16`
   changed the actual PTY to **50x16**.
4. Desktop focus caused `terminal.closed` / `terminal attach taken over`, exited
   the controller, restored **93x39**, and left the observer connected.
5. A fresh phone controller reacquired **50x16**. Desktop typing produced the same
   shutdown and **93x39** restoration; observer remained connected.
6. Two subsequent direct controllers demonstrated ordinary direct takeover.
7. Only the test's client PIDs were terminated. `session stop` and `session delete`
   succeeded; the isolated session list showed no named sessions afterward.

Earlier smoke harness attempts were also cleaned up. They first used the wrong
geometry evidence (observer dimensions), then encountered first-run onboarding;
using actual `stty size` and disabling onboarding only in the test config fixed
the harness. The final run above passed. No physical SSHuv/iPhone acceptance is
claimed; that remains the coordinator's device check after deployment.

## Coordinator deployment (NOT executed)

Run these commands on shuvdev. Set `HERDR_DEPLOY_SESSION` to the actual production
session to upgrade; `default` below is explicit, not inferred from this test.
This upgrades one server at a time. Keep other named sessions unchanged unless
separately selecting them.

```sh
set -eu
HERDR_DEPLOY_SESSION=default
HERDR_SHUV4="$HOME/src-scratch/herdr-shuv4/herdr-linux-x86_64-0.9.1-shuv.4"
printf '%s  %s\n' \
  '69d84185b186d2e274673a452bc51a5ed3410c130f78914a64f2d50a91dd9116' \
  "$HERDR_SHUV4" | sha256sum -c -
"$HERDR_SHUV4" --version

env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  "$HOME/.local/bin/herdr" --session "$HERDR_DEPLOY_SESSION" status server --json

# Preserve the previous binary and replace by rename, without truncating an
# executable that the current server might still have mapped.
HERDR_BACKUP="$HOME/.local/bin/herdr.before-shuv4.$(date +%Y%m%dT%H%M%S)"
cp -p "$HOME/.local/bin/herdr" "$HERDR_BACKUP"
HERDR_STAGE="$(mktemp "$HOME/.local/bin/herdr.shuv4.XXXXXX")"
install -m 0755 "$HERDR_SHUV4" "$HERDR_STAGE"
mv -f "$HERDR_STAGE" "$HOME/.local/bin/herdr"

env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  "$HOME/.local/bin/herdr" --session "$HERDR_DEPLOY_SESSION" server live-handoff \
  --import-exe "$HOME/.local/bin/herdr" \
  --expected-protocol 22 \
  --expected-version '0.9.1-shuv.4+63544697-shuv4-worktree'

env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  "$HOME/.local/bin/herdr" --session "$HERDR_DEPLOY_SESSION" status server --json
```

Confirm the *running server* version and capability after live handoff; a CLI
version alone is insufficient. If handoff fails, inspect the error and keep the
old server running; do not fall back to restart/stop. Then test SSHuv control,
desktop focus/typing, observer survival, and phone reacquisition on a disposable
pane before treating the phone UI as accepted.
