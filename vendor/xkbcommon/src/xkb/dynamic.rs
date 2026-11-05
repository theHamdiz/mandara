//! Mandara patch: CLI commands start before keyboard packages are installed.

use std::sync::LazyLock;

const LIBRARY_NAME: &str = "libxkbcommon.so.0";

/// Probe optional CLI work without forcing a keyboard operation to panic.
pub(crate) fn is_available() -> bool {
    static AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
        // The probe does not retain symbols or change the FFI invariant.
        unsafe { libloading::Library::new(LIBRARY_NAME) }.is_ok()
    });
    *AVAILABLE
}

static LIBRARY: LazyLock<libloading::Library> = LazyLock::new(|| {
    // The process retains the library for the lifetime of every symbol.
    unsafe { libloading::Library::new(LIBRARY_NAME) }
        .expect("libxkbcommon.so.0 is required for keyboard input")
});

/// `T` must match the unchanged libxkbcommon C declaration of `name`.
pub(super) unsafe fn symbol<T: Copy>(name: &[u8]) -> T {
    // The caller gives the C signature, and LIBRARY is never unloaded.
    unsafe { *LIBRARY.get::<T>(name).expect("libxkbcommon symbol is unavailable") }
}

// Keep the original FFI call sites, including its two C-variadic entries.
macro_rules! dynamic_xkb {
    () => {};
    (pub fn $name:ident($($arg:ident: $ty:ty,)* ...) $(-> $ret:ty)?; $($rest:tt)*) => {
        #[expect(non_upper_case_globals, reason = "Preserve the upstream C FFI names")]
        pub static $name: std::sync::LazyLock<unsafe extern "C" fn($($ty,)* ...) $(-> $ret)?> =
            std::sync::LazyLock::new(|| unsafe {
                $crate::xkb::ffi::dynamic::symbol(concat!(stringify!($name), "\0").as_bytes())
            });
        dynamic_xkb! { $($rest)* }
    };
    (pub fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?; $($rest:tt)*) => {
        #[expect(non_upper_case_globals, reason = "Preserve the upstream C FFI names")]
        pub static $name: std::sync::LazyLock<unsafe extern "C" fn($($ty),*) $(-> $ret)?> =
            std::sync::LazyLock::new(|| unsafe {
                $crate::xkb::ffi::dynamic::symbol(concat!(stringify!($name), "\0").as_bytes())
            });
        dynamic_xkb! { $($rest)* }
    };
}
