//! The system's CPU and C library, and the release builds that fit them.

/// Oldest Rust that builds Mandara (source builds update older toolchains).
pub const MIN_RUST: &str = "1.88.0";
/// CPU flags of the x86-64-v3 level (AVX2 era) the tuned build needs.
const X86_64_V3_FLAGS: &[&str] = &["avx2", "bmi1", "bmi2", "fma", "movbe", "f16c", "abm"];
/// Suffix of the x86-64-v3 tuned release (glibc only).
pub const V3_SUFFIX: &str = "-x86-64-v3";
/// CPU architectures with prebuilt releases.
pub const RELEASED_ARCHES: &[&str] = &["x86_64", "aarch64"];

/// The C library this binary links (`gnu` or `musl`): the system's, since
/// a dynamically linked binary only runs on its own libc.
pub fn compiled_libc() -> &'static str {
    if cfg!(target_env = "musl") { "musl" } else { "gnu" }
}

/// The release target this binary was built for, as release assets and
/// `update.json` name it (`x86_64-linux-gnu`, `x86_64-linux-gnu-x86-64-v3`,
/// `aarch64-linux-musl`…). Same rule as the updater's `running_target`.
pub fn compiled_target() -> String {
    let libc = compiled_libc();
    let v3 = libc == "gnu" && cfg!(all(target_arch = "x86_64", target_feature = "avx2", target_feature = "bmi2", target_feature = "fma", target_feature = "movbe"));
    format!("{}-linux-{libc}{}", std::env::consts::ARCH, if v3 { V3_SUFFIX } else { "" })
}

/// rustup's host triple for this system (`x86_64-unknown-linux-gnu`).
pub fn rust_triple() -> String {
    format!("{}-unknown-linux-{}", std::env::consts::ARCH, compiled_libc())
}

/// Whether `/proc/cpuinfo` text shows every x86-64-v3 flag.
pub fn cpu_supports_x86_64_v3(cpuinfo: &str) -> bool {
    let Some(flags) = cpuinfo.lines().find(|l| l.starts_with("flags")) else { return false };
    let flags: Vec<&str> = flags.split_once(':').map(|(_, f)| f).unwrap_or_default().split_whitespace().collect();
    X86_64_V3_FLAGS.iter().all(|f| flags.contains(f))
}

/// The release archives to try for `version` on this platform, best first
/// (file stems, without `.tar.gz`): the x86-64-v3 build when the CPU has
/// it (glibc only), then the baseline.
pub fn release_candidates(version: &str, arch: &str, libc: &str, v3: bool) -> Vec<String> {
    let base = format!("mandara-{version}-{arch}-linux-{libc}");
    let mut out = Vec::new();
    if v3 && arch == "x86_64" && libc == "gnu" {
        out.push(format!("{base}{V3_SUFFIX}"));
    }
    out.push(base);
    out
}

/// `a >= b` for dotted versions (`1.88.0`; missing parts count as 0,
/// anything after a `-` or `+` is ignored).
pub fn version_ge(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or_default().split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parse(a), parse(b));
    let len = a.len().max(b.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..len).map(|i| at(&a, i).cmp(&at(&b, i))).find(|o| o.is_ne()).is_none_or(|o| o.is_gt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_64_v3_needs_every_flag() {
        let all = "processor\t: 0\nflags\t\t: fpu sse2 avx2 bmi1 bmi2 fma movbe f16c abm\n";
        assert!(cpu_supports_x86_64_v3(all));
        assert!(!cpu_supports_x86_64_v3("flags\t\t: fpu sse2 avx2 bmi1 bmi2 fma f16c abm\n"));
        assert!(!cpu_supports_x86_64_v3(""));
    }

    #[test]
    fn candidates_prefer_the_tuned_build_on_glibc() {
        assert_eq!(release_candidates("0.2.0", "x86_64", "gnu", true), ["mandara-0.2.0-x86_64-linux-gnu-x86-64-v3", "mandara-0.2.0-x86_64-linux-gnu"]);
        assert_eq!(release_candidates("0.2.0", "x86_64", "musl", true), ["mandara-0.2.0-x86_64-linux-musl"]);
        assert_eq!(release_candidates("0.2.0", "aarch64", "gnu", true), ["mandara-0.2.0-aarch64-linux-gnu"]);
    }

    #[test]
    fn dotted_versions() {
        assert!(version_ge("1.88.0", MIN_RUST));
        assert!(version_ge("1.90.1", "1.88.0"));
        assert!(version_ge("1.88", "1.88.0"));
        assert!(!version_ge("1.87.9", "1.88.0"));
        assert!(version_ge("2.0.0-nightly", "1.88.0"));
        assert!(version_ge("2.39", "2.35"));
        assert!(!version_ge("2.31", "2.35"));
    }

    #[test]
    fn targets_name_this_build() {
        let t = compiled_target();
        assert!(t.starts_with(std::env::consts::ARCH) && t.contains("-linux-"), "{t}");
        assert!(rust_triple().contains("-unknown-linux-"));
    }
}
