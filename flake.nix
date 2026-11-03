{
  description = "Mandara Shell: a design-first Wayland desktop shell for MangoWM, Niri, Hyprland, Sway, River Classic and Umbriel";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # The `mandara` binary's version (crates/mandara-app/Cargo.toml).
      version = (lib.importTOML ./crates/mandara-app/Cargo.toml).package.version;
      rev = self.rev or (lib.removeSuffix "-dirty" (self.dirtyRev or "unknown"));
      # What the build reads: everything but the docs, CI and these Nix
      # files, so editing those in a checkout doesn't rebuild Mandara.
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.difference ./. (
          lib.fileset.unions [
            ./docs
            ./.github
            ./nix
            ./flake.nix
            ./flake.lock
          ]
        );
      };
      mandaraFor =
        pkgs:
        pkgs.callPackage ./nix/package.nix {
          inherit src version rev;
          inherit (self) lastModified;
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        mandara = mandaraFor pkgs;
        default = mandara;
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.mandara;
          meta.description = "Run Mandara Shell";
        };
      });

      overlays.default = final: _prev: { mandara = mandaraFor final; };

      nixosModules = rec {
        mandara = import ./nix/nixos-module.nix self;
        default = mandara;
      };

      homeManagerModules = rec {
        mandara = import ./nix/home-manager-module.nix self;
        default = mandara;
      };
      # The name newer Home Manager documentation uses.
      homeModules = self.homeManagerModules;

      checks = forAllSystems (
        pkgs:
        let
          system = pkgs.stdenv.hostPlatform.system;
          # A minimal NixOS system with the module and Mandara Login on. A
          # container system (no kernel, firmware or boot loader) and no
          # speech synthesis keep the check's download small; neither
          # touches what the module sets up.
          nixos = lib.nixosSystem {
            modules = [
              self.nixosModules.default
              {
                nixpkgs.hostPlatform = system;
                boot.isContainer = true;
                services.speechd.enable = false;
                programs.mandara = {
                  enable = true;
                  greeter.enable = true;
                  monitorBrightness.enable = true;
                };
                system.stateVersion = "25.11";
              }
            ];
          };
          # The Home Manager module against the few Home Manager options it
          # sets (declared here, so the check needs no Home Manager input).
          hm = lib.evalModules {
            modules = [
              self.homeManagerModules.default
              {
                options = {
                  home.packages = lib.mkOption { type = lib.types.listOf lib.types.package; };
                  xdg.configFile = lib.mkOption {
                    type = lib.types.attrsOf (
                      lib.types.submodule { options.source = lib.mkOption { type = lib.types.path; }; }
                    );
                    default = { };
                  };
                  systemd.user.services = lib.mkOption {
                    type = lib.types.attrsOf lib.types.anything;
                    default = { };
                  };
                };
                config = {
                  _module.args.pkgs = pkgs;
                  programs.mandara = {
                    enable = true;
                    settings.bar.height = 30;
                    systemd.enable = true;
                  };
                };
              }
            ];
          };
          service = hm.config.systemd.user.services.mandara;
        in
        {
          nixos-module = nixos.config.system.build.toplevel;
          home-manager-module = pkgs.runCommand "mandara-home-manager-module" { } ''
            grep -qx 'height = 30' ${hm.config.xdg.configFile."mandara/config.toml".source}
            test ${lib.escapeShellArg service.Service.ExecStart} = ${lib.getExe hm.config.programs.mandara.package}
            test ${toString (lib.length hm.config.home.packages)} = 1
            touch $out
          '';
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.mandara ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
          ];
          # `cargo run` builds load Vulkan, Wayland and libpam with dlopen;
          # the packaged binary has them on its RUNPATH instead.
          LD_LIBRARY_PATH = lib.makeLibraryPath (
            with pkgs;
            [
              vulkan-loader
              libGL
              wayland
              libxkbcommon
              linux-pam
            ]
          );
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
