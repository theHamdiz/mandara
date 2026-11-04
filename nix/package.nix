# Mandara Shell, built from source: the `mandara` binary (shell, Studio,
# lock screen, Mandara Login), the Studio desktop entry and icon, and the
# lock screen's PAM file as documentation (NixOS writes PAM services
# itself: `programs.mandara.enable` in ./nixos-module.nix).
{
  lib,
  rustPlatform,
  pkg-config,
  cmake,
  wayland,
  libxkbcommon,
  vulkan-loader,
  libGL,
  fontconfig,
  freetype,
  linux-pam,
  # The flake's source tree, its revision (`self.rev`, or "unknown") and
  # last commit time (`self.lastModified`, Unix seconds).
  src,
  version,
  rev ? "unknown",
  lastModified ? null,
}:

rustPlatform.buildRustPackage {
  pname = "mandara";
  inherit version src;

  cargoLock = {
    lockFile = ../Cargo.lock;
    # One hash per git revision in Cargo.lock: GPUI's repository and the
    # crates its workspace patches (vendor/gpui and vendor/gpui_wgpu are
    # path crates). Update them when those revisions change in Cargo.lock.
    outputHashes = {
      "collections-0.1.0" = "sha256-EwAeBfrL6VnboHo/uBG//+7RGQqHG2N2psVakeHvpvY=";
      "async-task-4.7.1" = "sha256-503Yewi444XtYodEWIggMmVN3NKqCgtMBNegiMyJcCI=";
      "calloop-0.14.3" = "sha256-5lWGGm6Wehf3RNFnhll0hMlUCXeVFHeoyHN4pbMS9zQ=";
      "khronos-egl-6.0.0" = "sha256-VeDtEMFI7C9b5INaIIVI3AO9akPxXiE11TiM3Gw/knU=";
      "naga-30.0.0" = "sha256-ZLW6cQINooN20HUyRQj5BPnc7RODgcsbOOAzcGJEsM4=";
      "oo7-0.6.0" = "sha256-QXblWsnq+graWOSW0iA8pUzpwizDLi/kOSWDdSQVZ/0=";
      "zed-font-kit-0.14.1-zed" = "sha256-KXygi0olNQi5yM8eaJVykNDtbPMDjT+cWPBF8UrtXR4=";
    };
  };

  nativeBuildInputs = [
    pkg-config
    cmake
  ];

  buildInputs = [
    wayland
    libxkbcommon
    vulkan-loader
    fontconfig
    freetype
    linux-pam
  ];

  # The `mandara` binary, plus `xtask` to compile the language packs into
  # the output (it's removed afterwards); the workspace's tests run in CI
  # (several need a headless Wayland sandbox).
  cargoBuildFlags = [
    "--package"
    "mandara-app"
    "--package"
    "xtask"
  ];
  doCheck = false;

  # The `[profile.release]` in Cargo.toml (fat LTO, one codegen unit,
  # unwinding panics) is what `buildRustPackage` builds with. The git
  # revision and build date `mandara --version` shows come from here: the
  # source in the store has no `.git` (crates/mandara-version/build.rs).
  env = {
    GITHUB_SHA = rev;
  }
  // lib.optionalAttrs (lastModified != null) { SOURCE_DATE_EPOCH = toString lastModified; };

  # GPUI loads Vulkan (and EGL as a fallback) and Wayland with dlopen, and
  # the lock screen loads libpam the same way: put them on the binary's
  # RUNPATH instead of a wrapper's LD_LIBRARY_PATH, which would leak into
  # every app Mandara launches. Vulkan drivers come from the system
  # (`hardware.graphics` on NixOS).
  postFixup = ''
    patchelf --add-rpath ${
      lib.makeLibraryPath [
        vulkan-loader
        libGL
        wayland
        libxkbcommon
        linux-pam
      ]
    } $out/bin/mandara
  '';

  postInstall = ''
    # Every language pack for exactly this build's version, where the shell
    # looks first after the user's own data folder (<exe>/../share/…).
    v=$($out/bin/mandara --version | cut -d' ' -f2)
    $out/bin/xtask langpacks --version "$v" --dir $out/share/mandara/locales/$v
    rm $out/bin/xtask
    install -Dm644 packaging/org.mandara.studio.desktop -t $out/share/applications
    # The main menu's calligraphy is drawn from Reem Kufi (OFL).
    install -Dm644 crates/mandara-ui/assets/calligraphy/OFL.txt $out/share/licenses/mandara/OFL.txt
    install -Dm644 packaging/org.mandara.studio.svg -t $out/share/icons/hicolor/scalable/apps
    install -Dm644 packaging/pam/mandara-lock -t $out/share/doc/mandara/pam.d
    substituteInPlace $out/share/applications/org.mandara.studio.desktop \
      --replace-fail "Exec=mandara studio" "Exec=$out/bin/mandara studio"
  '';

  meta = {
    description = "Design-first Wayland desktop shell for MangoWM, Niri, Hyprland, Umbriel, Sway and River Classic";
    homepage = "https://github.com/theHamdiz/mandara";
    changelog = "https://github.com/theHamdiz/mandara/blob/main/CHANGELOG.md";
    license = lib.licenses.gpl3Plus;
    mainProgram = "mandara";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
