//! Linker options shared with the build-policy regression tests.

/// First glibc whose dynamic loader handles RELR (`GLIBC_ABI_DT_RELR`).
const RELR_MIN_GLIBC: (u32, u32) = (2, 36);

/// GNU ld supports `-z pack-relative-relocs` on i386 and x86-64.
pub fn packed_relocations(os: &str, env: &str, arch: &str, glibc: Option<(u32, u32)>) -> bool {
    os == "linux" && env == "gnu" && matches!(arch, "x86" | "x86_64") && glibc.is_some_and(|version| version >= RELR_MIN_GLIBC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_relocations_require_both_linker_and_loader_support() {
        assert!(packed_relocations("linux", "gnu", "x86_64", Some((2, 36))));
        assert!(packed_relocations("linux", "gnu", "x86", Some((2, 44))));
        assert!(!packed_relocations("linux", "gnu", "aarch64", Some((2, 44))));
        assert!(!packed_relocations("linux", "gnu", "x86_64", Some((2, 35))));
        assert!(!packed_relocations("linux", "musl", "x86_64", None));
        assert!(!packed_relocations("freebsd", "", "x86_64", None));
    }
}
