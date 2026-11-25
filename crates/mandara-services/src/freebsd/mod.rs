//! FreeBSD's own system interfaces, used where Linux reads procfs, sysfs
//! or a Linux-only daemon: sysctl(3) for processes, memory, CPU time and
//! sensors, ACPI battery nodes, OSS `mixer(8)` and `/dev/dsp`, `ifconfig(8)` and
//! `wpa_cli(8)`, and backlight(9) devices.
//!
//! Each submodule keeps its decoders platform-neutral (tested on every
//! build host) and its system calls behind `cfg(target_os = "freebsd")`.

pub mod acpi;
pub mod backlight;
pub mod dsp;
pub mod host;
pub mod mounts;
pub mod net;
pub mod oss;
pub mod power;
pub mod procs;
#[cfg(target_os = "freebsd")]
pub mod sysctl;
