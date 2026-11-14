{ self, lib, pkgs }:
let
  host = pkgs.extend (_final: prev: { mandaraHostProbe = prev.hello; });
  probe = self // {
    packages = throw "Mandara modules must use the host package set";
    overlays.default = final: _prev: { mandara = final.mandaraHostProbe; };
  };
  system = enabled: package: lib.nixosSystem {
    modules = [
      (import ../nixos-module.nix probe)
      {
        nixpkgs.pkgs = host;
        boot.isContainer = true;
        boot.kernelPackages = host.linuxPackages_latest;
        boot.kernelModules = [ "iwlwifi" "btusb" ];
        boot.extraModulePackages = [ host.linuxPackages_latest.v4l2loopback ];
        hardware.enableRedistributableFirmware = false;
        hardware.firmware = [ host.wireless-regdb ];
        hardware.bluetooth.enable = true;
        networking.networkmanager.enable = true;
        programs.mandara = { enable = enabled; } // package;
        system.stateVersion = "25.11";
      }
    ];
  };
  before = (system false { }).config;
  after = (system true { }).config;
  home = package: lib.evalModules {
    modules = [
      (import ../home-manager-module.nix probe)
      {
        options = {
          home.packages = lib.mkOption { type = lib.types.listOf lib.types.package; };
          xdg.configFile = lib.mkOption { type = lib.types.attrsOf lib.types.anything; default = { }; };
          systemd.user.services = lib.mkOption { type = lib.types.attrsOf lib.types.anything; default = { }; };
        };
        config = {
          _module.args.pkgs = host;
          programs.mandara = { enable = true; } // package;
        };
      }
    ];
  };
  paths = builtins.map (package: package.drvPath);
in
assert after.programs.mandara.package.drvPath == host.mandaraHostProbe.drvPath;
assert (system true { package = host.coreutils; }).config.programs.mandara.package.drvPath == host.coreutils.drvPath;
assert (home { }).config.programs.mandara.package.drvPath == host.mandaraHostProbe.drvPath;
assert (home { package = host.coreutils; }).config.programs.mandara.package.drvPath == host.coreutils.drvPath;
assert before.boot.kernelPackages.kernel.drvPath == after.boot.kernelPackages.kernel.drvPath;
assert paths before.boot.extraModulePackages == paths after.boot.extraModulePackages;
assert before.boot.kernelModules == after.boot.kernelModules;
assert before.hardware.firmware.drvPath == after.hardware.firmware.drvPath;
assert before.networking.networkmanager.package.drvPath == after.networking.networkmanager.package.drvPath;
assert before.hardware.bluetooth.package.drvPath == after.hardware.bluetooth.package.drvPath;
assert after.nixpkgs.pkgs.mandaraHostProbe.drvPath == host.mandaraHostProbe.drvPath;
pkgs.writeText "mandara-host-packages" "checked\n"
