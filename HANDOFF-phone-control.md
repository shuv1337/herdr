# Phone direct-control reclaim handoff

Updated 2026-09-27 in `/Users/shuv/repos/herdr-phone-handoff`, branch
`direct-control-reclaim-fix`, based on `96608fdc`; code commit `ca54373c`.
Fork remote:
`Latitudes-Dev/herdr`. This is a local source fix. No push, tag, release,
installation, server restart, live handoff, or physical phone acceptance was
performed.

## Corrected behavior

- A ClientShell focus report reclaims a pane only on that client's established
  `Some(false) -> true` transition. The first report after connection and the
  first report after every `surface.set(true)` establishes a baseline, even if
  it says `true`. Repeated `true` never reclaims.
- Pane and popup input reclaims only on key press/repeat, text commit, paste,
  or mouse down. Hover, drag, scroll, key release, and mouse up retain the
  direct controller. The broader interaction check still promotes foreground
  clients as before.
- ClientShell API requests reclaim only after a successful explicit pane,
  direction, agent, tab, or workspace focus method. Incidental focus changes
  caused by close, swap, and other methods do not reclaim.
- Popup input and popup image-path paste reclaim the popup terminal when the
  active shell owns the popup's tab. Stale, inactive, and non-owner requests
  are still rejected. All reclaim paths decline during live handoff.
- Reclaim sends the existing `terminal attach taken over` shutdown, removes
  the direct owner and resize lock, then claims the reclaiming shell's tab
  geometry. It skips the former shell controller's intermediate restore, so
  reclaim does not resize the pane twice. Observers stay connected. Normal
  direct-to-direct takeover keeps its existing geometry behavior.

`direct_control_yields_to_shell` is a server capability. It remains in the
server JSON capability object and the server's endpoint welcome list, but no
longer appears in `status client --json` as a local client capability. This is
an optional capability-list correction; endpoint generation 1, protocol 22,
and frozen codecs are unchanged.

## Fork version

The next fork release identity is **0.9.1-shuv.5**. Cargo's base version stays
`0.9.1`, following the fork convention. Build with `HERDR_FORK_REVISION=5`
when preparing a fork artifact. This handoff does not create a tag or artifact
for shuv.5. The previous shuv.4 binary and its 2026-09-26 smoke results were
built before this correction and must not be treated as containing it.

## Validation

All heavy Cargo commands on this Mac ran through `/tmp/sshuv-heavy.sh`; set
`ZIG=/Users/shuv/.local/opt/zig-aarch64-macos-0.16.0/zig` because the default
`zig` is 0.15.2.

- Focused `cargo test --locked direct_control -- --test-threads=1`: **11 passed**.
- Full plain `cargo test --locked`: **incomplete**. The test process ended in
  SIGPIPE from `platform::remote_bridge_tests::bridge_child` while other tests
  were running; it did not produce a valid full-suite count. See
  `/tmp/herdr-reclaim-full-test.log`. The earlier shuv.4 handoff recorded the
  same harness failure mode. Full `cargo nextest run --locked`, the project's
  default suite runner, is queued under the shared lock. At this handoff the
  lock was held by another Xcode UI test for over an hour, so Nextest had not
  started. Its output is directed to `/tmp/herdr-reclaim-nextest.log`.
- `cargo clippy --all-targets --locked -- -D warnings`: **passed**.
- `just maintenance-test`: **158 Python and 5 Bun tests passed**.
- `just ui-hot-path-architecture-test`: **6 passed**.
- `just integration-assets-test`: **47 Bun tests passed**.
- `just docs-contract-test`: **7 passed**.
- `cargo fmt --check` and `git diff --check`: **passed**.
- `HERDR_FORK_REVISION=5 cargo build --release`: **passed**. The local artifact
  is `target/release/herdr`, Mach-O arm64, SHA-256
  `753efbb152e38e40d7e0f3e7e2f86549f73710ae42cdaedd992ecadc9c8292b9`.
  It reports `herdr 0.9.1-shuv.5+96608fdc6ca6`. The embedded commit points
  to the pre-fix branch head because the build preceded the local commits; the
  worktree source included the correction. This artifact is validation output,
  not a Linux deployment binary.

## Coordinator handoff

The coordinator can review these local commits, build a separate Linux fork
artifact with `HERDR_FORK_REVISION=5`, and choose any deployment and live test
later. A release binary built on this Mac is a macOS artifact, not the Linux
server binary. Before accepting phone behavior, verify the running server's
version and capability and exercise phone takeover, desktop focus/typing,
popup input, geometry, observer continuity, and phone reacquisition in a
disposable pane. Those live and physical checks remain outstanding.
