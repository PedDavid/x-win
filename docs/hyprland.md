# Hyprland backend (standalone fork)

This implements Hyprland-specific IPC, related to upstream issue #70. It is **not**
generic Wayland support. No time-tracker integration or terminal attribution is
included. Public Rust and Node window shapes and exports are unchanged.

## Source and tested environment

- Upstream refreshed from `https://github.com/miniben-90/x-win` at
  `a6ebeae999de053e7d5a08656898d79baee77b19` (HEAD at implementation time).
- Implementation branch: `feat/hyprland-ipc`; see its Git history for the patch revision.
- Live: Hyprland **v0.56.2**, compositor revision
  `efb50993780079460b0cbed1363e2166a2de1d9f`, Linux x86_64 GNU.
- Rust/cargo 1.99.1; Node v26.13.0. Upstream CI uses stable Rust; there is no
  pinned rust-toolchain file. The manifest's old MSRV is not re-certified here
  (existing image/zbus/N-API dependencies require a much newer toolchain).
- Yarn 4.18.1 is bundled; `yarn.lock` is unchanged. No JS packages were installed.
- No contribution/AGENTS instructions exist in this checkout.

### Compatibility envelope

The discovery layout is the runtime-directory layout used by Hyprland 0.40+
(`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`). Only 0.56.2
has been live-tested; do not infer a tested guarantee for every older release.
Older `/tmp/hypr` layouts are deliberately unsupported. Required capability:
`j/clients` and `j/activewindow` with nonzero hexadecimal `address` values.

Reference: <https://wiki.hypr.land/IPC/> and v0.56.2
`src/debug/HyprCtl.cpp`. Commands are raw `j/clients` or `j/activewindow`, **without**
a newline or a length prefix. The response ends when the server closes its side.
The connection is opened immediately before writing and always dropped after
one request, including on error. No `hyprctl` or other subprocess runs per poll.

## Routing

Hyprland desktop context (case-insensitive colon-delimited
`XDG_CURRENT_DESKTOP`) or a nonempty instance signature selects Hyprland, even
when the socket is temporarily missing. Missing context/runtime is an error,
not a fallback to unrelated X11 windows. GNOME Wayland continues to use the
existing GNOME implementation; non-Wayland sessions use existing X11. Unknown
Wayland compositors return an explicit unsupported-compositor error instead of
instructions to install a GNOME extension. GNOME extension management returns
`false` outside the GNOME Wayland backend, without changing settings.

The runtime directory must be absolute; the instance signature must be a single
normal path component. No global socket search or `/tmp` fallback. This trusts
the process's session environment, as does Hyprland's own client; callers must
not supply untrusted environment variables or run this as root.

## Field contract

| Public field | Hyprland mapping |
| --- | --- |
| `id` | Positive process-local allocated window ID; zero reserved for no focus |
| `title` | Compositor `title`, empty if unavailable |
| `info.name` | Compositor application `class`, not an inferred display name |
| `info.process_id` / Node `processId` | Window owner's compositor PID, zero if invalid/missing |
| `info.path`, `info.exec_name` / Node `execName` | Best-effort `/proc/<pid>/exe`, empty on any failure |
| `position.x/y` | Compositor global logical coordinates (may be negative) |
| `position.width/height` | Compositor logical size, zero if missing/invalid; negative sizes clamped to zero |
| `position.is_full_screen` / Node `isFullScreen` | Numeric `fullscreen` bit 2; boolean accepted for older schemas; maximized mode 1 is not fullscreen |
| `usage.memory` | Existing Linux helper's **virtual memory in pages**, first `/proc/<pid>/statm` column; NOT RSS or bytes |
| `os` | `linux` |

Memory remains process-level and may be shared by multiple windows. This patch
does not change cross-platform units. `fullscreenClient` is not substituted for
the compositor's actual fullscreen mode. Optional `stableId` is not required.
Unknown JSON fields and the native/XWayland distinction do not alter mapping.
An unreadable, missing, or exited process never suppresses compositor metadata.
Only executable symlink and statm are inspected: no argv, environment, terminal
content, browser-title URL inference, or project lookup.

Icons return the existing empty icon shape (empty data, zero width/height).
Browser URL returns the existing Linux unsupported-message string. `toObject()`
continues to work with these graceful results.

`{}` from activewindow returns the existing empty entity, including `id = 0` and
`os = linux`. A clients response must be an array; bad JSON, invalid/duplicate
addresses, nonobject active responses, oversized or truncated messages are
errors. Missing optional metadata gets defaults. A focused address absent from
the immediately preceding clients snapshot is treated as no focus (close/open
race), to be retried on the next poll.

## Identity, disconnects, and visibility

Each active poll first obtains clients, prunes closed identities, then queries
activewindow. Open-window queries also reconcile identities. IDs are allocated
monotonically, **not** from PID, truncated pointers, or unchecked hashes.
Title changes preserve ID. Concurrent windows with the same PID are distinct.
Observed close/reopen, changed PID or optional stableId, socket errors, malformed
responses, or changed instance path invalidate identity. Numeric IDs are never
reused during this library instance's lifetime; exhaustion returns an error.
Applications must not persist these IDs across library/process restarts.

**Polling limit:** a close and address reuse entirely between snapshots cannot
be reliably detected on versions without stableId, particularly if PID is also
unchanged. That is indistinguishable through snapshots alone. A later event
backend could close that gap; this implementation does not claim event-level
identity guarantees. The mapping has one entry per currently observed client;
the counter, not historical entries, survives reconnect.

There is no cached WindowInfo fallback on errors. The next query reconnects
from scratch. Each request has a **150 ms total deadline** (nonblocking connect,
write, and all reads, including trickle responses), and a **4 MiB** response cap.
An active query uses at most two such deadlines, plus bounded parsing and local
proc work. Linux proc filesystem reads are best-effort ordinary file operations,
not independently timed. Concurrent access uses try_lock rather than an
unbounded wait behind another compositor query; contention is a recoverable
query error. No socket remains held between polls.

**Lock limitation / acceptance gap:** request IPC has no reliable session-lock
flag. This backend returns only what activewindow reports *now*, not a cached
pre-lock result. If Hyprland reports `{}` while locked, consumers get no focus.
If it continues to report its previous focus while a lock surface owns keyboard
input, this library cannot distinguish it. It is **not idle/lock detection**;
consumers needing that guarantee must separately gate activity using an
authoritative session-lock signal. Actual lock/unlock was not exercised (no
desktop state/configuration changes were requested). This remains an explicit
Phase 1 acceptance limitation, not an assertion that locking is handled.

## Node subscription policy (shared change)

Polling remains the subscription mechanism. The shared policy is isolated in
`src/common/subscription.rs` and applies equally to all backends:

- Deliver first successful snapshot, including initial no focus.
- Deliver ID or title changes, including focus loss and restoration.
- On first error in an outage, send an empty-window update to invalidate stale
  activity, then an error callback (`info` undefined). Do not terminate.
- Retry at max(requested interval, 250 ms); suppress duplicate outage errors.
- First success after an error is always delivered, even if ID/title match.
- Cancellation interrupts the interval wait (even a 60-second interval), and
  callbacks use nonblocking queue submission. A running Hyprland query finishes
  within its request budget; callbacks already queued may arrive after cancel.
- `unsubscribe` remains signal-based, not a join/flush guarantee. Blocking
  behavior inside other platforms' existing snapshot implementations is not
  fixed by this patch.

Consumers should treat no focus and errors as no attributable activity. This
is an intentional change from permanently ending subscriptions on any error.

## Reproduction, without installing anything

From the checkout root, with existing stable Rust, Linux build dependencies and
Node available (do not install system or JS dependencies automatically):

```sh
cargo test --manifest-path x-win-rs/Cargo.toml
cargo test --lib
cargo clippy --manifest-path x-win-rs/Cargo.toml --all-features -- -D warnings
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --manifest-path x-win-rs/Cargo.toml -- --check
cargo fmt -- --check

# Compositor-independent native Node integration:
cargo build
cp target/debug/libx_win_napi.so target/debug/x-win.node
node examples/hyprland-subscription-test.cjs target/debug/x-win.node

# LIVE integration: must run in the intended Hyprland session:
cargo run --manifest-path x-win-rs/Cargo.toml --example hyprland_smoke
node examples/hyprland-smoke.cjs target/debug/x-win.node
```

The Node examples load the supplied absolute local native artifact directly,
not `index.js`'s platform-package fallback. The cargo build and `.so`→`.node`
copy are a dependency-free N-API build path on Linux, not the Yarn CLI build.
If JS dependencies are later approved, use the existing manager/lockfile:
`node .yarn/releases/yarn-4.18.1.cjs install --immutable`, then `yarn lint`,
`yarn build:debug`. Do not run publishing scripts. Existing AVA tests require
an interactive desktop and nonempty icons, so their icon assertions are not
appropriate for the Hyprland empty-icon contract.

Rust has no committed Cargo.lock upstream (both locks are ignored). Cargo
created local locks during baseline/build; subsequent runs can use `--locked`.
Do not copy the Node package's yarn.lock semantics onto Rust's library lockfile
policy. Local lock hashes are recorded in `hyprland-validation.md`.

Four preexisting live Rust tests are now explicitly `#[ignore]` so headless unit
tests are independent of the desktop. Run focus/list tests selectively, not
`--ignored` wholesale (that also includes extension-mutating tests):

```sh
cargo test --manifest-path x-win-rs/Cargo.toml tests::test_get_active_window -- --ignored --exact
cargo test --manifest-path x-win-rs/Cargo.toml tests::test_get_open_windows -- --ignored --exact
```

The old live icon tests require a backend with icon support, not Hyprland.
The examples log only sanitized counts, timings, and booleans. The synthetic
Node fixture temporarily sets its *own process's* environment and serves a
private temporary socket from a worker; it never contacts the real compositor.

## Upstream PR draft (not published)

**Title:** Linux: add Hyprland request-IPC window backend

Related to #70, not a generic Wayland implementation. Add context-based routing,
bounded direct Unix-socket queries, window metadata and collision-free identity,
best-effort proc metadata, and empty-icon compatibility. Preserve GNOME/X11
implementations and public data types. Separately isolate subscription recovery
and interruptible cancellation. Include synthetic transport/parser/identity and
native Node subscription tests plus sanitized live smoke examples.

Live-tested on Hyprland 0.56.2. Review points: polling address-reuse ambiguity,
session-lock visibility gap, GNOME context detection, shared subscription retry
policy, unsupported icons, and lack of older-version/cross-platform live tests.
No optional terminal attribution, installs, settings changes, publication,
telemetry, or persistence are included.
