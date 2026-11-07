# PRD 003 validation record

## Revision and scope

Base: upstream `a6ebeae999de053e7d5a08656898d79baee77b19`, refreshed by
`git ls-remote` and a fresh clone. Local branch `feat/hyprland-ipc`.
At validation time, no implementation commit, remote fork/PR, installation,
desktop configuration, application integration, or publication had been performed.
The implementation was subsequently committed and pushed to the user's GitHub
fork on request; no upstream PR was opened. Cargo fetched ordinary
build dependencies; no system packages or JS packages were installed.
Optional terminal attribution (Phases 2/3) was not started.

Read the upstream README, manifests, package scripts, Cargo configuration,
Linux API implementations, public types, Node lifecycle code, and CI/test
configuration. No separate contribution guide exists.

## Baseline, before implementation

| Command | Result |
| --- | --- |
| `cargo test --manifest-path x-win-rs/Cargo.toml` | 7 passed, 4 failed, 8 ignored |
| `cargo check` | Passed (root N-API) |

The four failures were active window, open windows, and two icon tests. All
reported missing GNOME extension on Hyprland, not compilation failures.

A separate pristine detached worktree at the same base revision also reproduced
`cargo clippy --all-features --all-targets -- -D warnings` failing with seven
preexisting test-only lints: one needless borrow and six boolean assert
comparisons. This patch deliberately does not mix in that cleanup.

## Implementation validation

| Check | Result |
| --- | --- |
| Rust library `cargo test --locked` | **16 passed**, 12 ignored, 0 failed; no doc tests |
| Root `cargo test --lib --locked` | **2 passed**, 0 failed |
| Library clippy, `--all-features -- -D warnings` | Passed |
| Root clippy, `--all-features --all-targets -- -D warnings` | Passed |
| Library clippy including `--all-targets` | Same seven preexisting test lints as pristine base |
| Rust formatting, both manifests | Passed |
| `git diff --check` | Passed |
| `cargo build --locked` | Passed, freshly linked native N-API `.so` |
| Node syntax checks for both new consumers | Passed |
| Native Node synthetic socket/subscription test | Passed |
| Yarn lint, AVA, and `napi build --platform` | Not run: JS dependencies absent; no install authorized |

The native `.so` was copied to `target/debug/x-win.node` and required directly
by the Node scripts. No installed platform package was used. Native linking,
exports, WindowInfo getters, toObject, and callback behavior were exercised
through this actual artifact; this is distinct from merely cargo-checking N-API.

### Synthetic Rust coverage

- Metadata parsing and defaults, missing/unknown fields, malformed JSON.
- Permission-denied executable lookup and exited-process memory lookup;
  successful synthetic proc metadata and unchanged memory units.
- Boolean/numeric fullscreen modes, maximized versus full screen, XWayland.
- Same PID and same low 32 address bits still produce different IDs.
- Title stability, observed-close cleanup, reconnect, optional stableId reuse,
  duplicate addresses, ID exhaustion.
- Safe runtime/signature construction; no fallback search.
- Unix socket normal response, EOF framing, malformed/truncated data, timeout,
  response cap, empty disconnect, slow trickle exceeding total deadline.
- Missing socket followed by malformed response followed by successful retry.
- Full snapshot roundtrips through a mock socket, no focus, close race,
  invalidation on failure and identity restoration.
- GNOME/X11/Hyprland/unsupported routing (pure tests, no environment mutation).

### Native Node coverage

A worker serves only synthetic window data in a private temporary directory.
The test exercises two windows sharing PID; missing proc; initial delivery;
title-only change with stable ID; no focus; restoration; malformed IPC outage
with explicit empty update/error; successful retry without resubscribing; no
new callbacks after cancellation settles; and ten repeated subscription cycles
with 60-second intervals. Cleanup terminates the worker and removes fixtures.

Root Rust tests separately cover duplicate suppression, first empty delivery,
recovery, ten retry/cancel loops, and cancellation of an hour-long interval in
less than 200 ms. Real hung-socket cancellation is bounded by the socket budget,
not proven by the fast mock-poll cancellation test alone.

## Live Hyprland checks

Environment: Hyprland v0.56.2 at
`efb50993780079460b0cbed1363e2166a2de1d9f`, x86_64 Linux GNU,
Rust 1.99.1 / cargo 1.99.1, Node v26.13.0.

- Rust example: valid focused snapshot, distinct nonzero IDs for open windows,
  graceful empty icon; 100 active queries averaged **248 microseconds**.
- Node example: valid focused snapshot and 3 open windows (window count varies
  between runs); getters and toObject succeeded.
- Node 100-query sample: **0.261 ms median**, **0.396 ms p95**, **0.477 ms max**.
- Node 100 ms subscription: initial delivery, **0 errors** over 1.50 seconds;
  approximate process CPU **0.38% of one CPU**, including native polling threads.
- Live preexisting active-window and open-window tests passed when explicitly
  run with `--ignored --exact`.

These are short **debug-build** samples, not a long-duration benchmark or a
compositor CPU measurement. The query sample is a tight burst; the separate
subscription CPU sample uses the actual 100 ms interval. No additional event
loop/cache architecture is justified by this initial measurement. Live output
was limited to counts, booleans, timing and artifact path; no private titles,
classes, command lines or executable paths were saved.

## Not verified / remaining limitations

- No real desktop lock/unlock, deliberate compositor restart, focus switching,
  or title manipulation was initiated. Transitions and reconnect are synthetic;
  live demonstration verifies snapshots and initial callback delivery only.
- **Session lock is not reliably observable through snapshot IPC.** A compositor
  retaining last focus behind a lock surface can still expose that window.
  This is an explicit PRD acceptance gap; do not use this backend alone as an
  idle/lock detector. See the contract notes for details.
- Close/address reuse entirely between snapshots can be indistinguishable on
  older versions without stableId; only observed lifecycle transitions can be
  guaranteed by polling.
- Actual GNOME/X11 desktops, older Hyprland releases, Windows/macOS, Linux musl,
  and other Node versions were unavailable/not exercised. Routing and shared
  polling policy have synthetic tests; existing backend implementations remain
  unchanged. No claim of complete cross-platform regression validation.
- Empty icons are supported behavior; the old live nonempty-icon tests are
  intentionally not applicable on Hyprland.
- Unsubscribe signals cancellation rather than joining/flushing callbacks;
  already-queued deliveries may still arrive. Other backends' blocking query
  implementations are outside this patch's bounded-IPC guarantee.
- JS package lint/AVA/CLI-native build remain unverified until dependencies are
  approved. The standalone native consumer does not depend on those packages.

## Local lockfile fingerprints

Cargo.lock files are generated but ignored according to upstream policy; no
new lockfile or toolchain policy was introduced. SHA-256 at validation:

```text
843d22f7c5d36e9ecbd6a5b0f0f7f2ebec6ee3652724155a868390aa7c8371b5  Cargo.lock
03c026b53b5d034cc25f0135edafb81b8041fd0f3f1726803faa334e30124eeb  x-win-rs/Cargo.lock
573467684a064f7f8b4ed037215e0d38d3c901cbee34b3e296f7f29050c99958  yarn.lock
```

Reproducible standalone commands and an unsubmitted upstream PR draft are in
[hyprland.md](hyprland.md). The implementation should be reviewed with the
remaining acceptance gaps above, not presented as generic Wayland completion.
