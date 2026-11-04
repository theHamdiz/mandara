# Mandara patches to xkbcommon 0.8.0

The Rust API is the version used by GPUI. The MIT-licensed upstream sources
are unchanged except for the deferred FFI binding, its `libloading`
dependency and explicit iterator lifetimes required by current Rust lints.

`src/xkb/ffi.rs` and `src/xkb/dynamic.rs` load `libxkbcommon.so.0` on the
first keyboard call, retaining the library and each resolved function for
the process lifetime. CLI commands can therefore start on a fresh Linux
system, so `mandara install` can install the keyboard package itself. The
original mandatory ELF dependency prevented the installer from starting.

The C declarations, argument types, ownership and safe Rust wrappers stay
the same. Symbols are resolved individually, so unused APIs do not impose
new requirements on older system libraries. Compose uses the same loader;
the unused X11 feature retains its upstream bindings.

The application’s loader regression test checks its ELF dependencies, and
the config and session-lock tests exercise the keyboard wrappers. A clean
container also checks the installer before the keyboard package exists.
