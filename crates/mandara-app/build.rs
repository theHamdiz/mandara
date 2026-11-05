//! Link flags for the `mandara` binary.

mod build_support;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    if build_support::packed_relocations(&os, &env, &arch, build_glibc()) {
        // RELR: the ~176 000 relative relocations of the position-independent
        // binary packed into a bitmap (binutils ≥ 2.38). The 4 MiB
        // `.rela.dyn` table the dynamic loader read (and kept resident) at
        // every start shrinks to a few KiB. Only where the glibc we link
        // against has a loader that understands it: rust-lld (the x86_64
        // default) marks the binary as needing `GLIBC_ABI_DT_RELR`, which an
        // older glibc refuses to load, so release builds made on the oldest
        // supported glibc stay portable.
        println!("cargo:rustc-link-arg-bins=-Wl,-z,pack-relative-relocs");
    }
    if os == "linux" {
        // Functions LLVM knows are cold (`#[cold]`, panic and error paths,
        // functions that only lead there) are emitted in `.text.unlikely.*`
        // sections; keep them in their own output section instead of
        // interleaved with live code, so the pages the running shell maps
        // hold more of the code it runs (2.2 MB moved out of `.text`).
        println!("cargo:rustc-link-arg-bins=-Wl,-z,keep-text-section-prefix");
    }
}

/// (major, minor) of the glibc this build links against: the build host's,
/// as native builds are the only glibc builds Mandara makes.
#[cfg(target_env = "gnu")]
fn build_glibc() -> Option<(u32, u32)> {
    // SAFETY: gnu_get_libc_version returns a static NUL-terminated string.
    let v = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }.to_str().ok()?;
    let mut parts = v.split('.').map(|p| p.parse().ok());
    Some((parts.next()??, parts.next()??))
}

#[cfg(not(target_env = "gnu"))]
fn build_glibc() -> Option<(u32, u32)> {
    None
}
