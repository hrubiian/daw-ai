# CPAL 0.15.3 ALSA shutdown patch

This directory contains CPAL 0.15.3 from crates.io, under its original
Apache-2.0 license. The workspace selects it through `[patch.crates-io]`.
Do not edit the machine's Cargo registry copy: that would not fix other builds.

The ALSA worker used to own the only read end of its shutdown pipe. If the
worker exited or panicked, dropping the stream wrote to a closed pipe and
asserted `-1 == 8`, then also unwrapped the failed thread join. The patch:

- Keeps a read-end reference until stream teardown finishes.
- Retries pipe reads/writes interrupted by a signal and returns other I/O errors.
- Joins failed workers without panicking again in the destructor.
- Reports worker panics through the existing stream error callback.

The shutdown code is isolated in `src/host/alsa/trigger.rs` and tested without
opening sound devices. Other backends and the public CPAL API are unchanged.
`crates/daw-io/tests/alsa_shutdown.rs` includes that same implementation so its
regressions also run with the normal workspace tests.

Upstream example-only dev dependencies and example auto-discovery are disabled
in the packaged manifest so these library tests use the existing workspace
lockfile without fetching unrelated demo dependencies. `Cargo.toml.orig` keeps
the upstream manifest for reference.

Run: `cargo test -p cpal --lib` and `cargo test --workspace`.

Upstream also retains the receiver and retries interrupted transfers in its
current ALSA implementation:
https://docs.rs/cpal/latest/src/cpal/host/alsa/mod.rs.html
