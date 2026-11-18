#![cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
))]

// Exercise the exact vendored shutdown implementation in normal workspace CI,
// without opening a PCM device or exposing backend internals in CPAL's API.
#[path = "../../../vendor/cpal/src/host/alsa/trigger.rs"]
mod trigger;
