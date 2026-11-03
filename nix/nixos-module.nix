# NixOS module: `programs.mandara.enable = true;` sets up, system-wide, what
# `mandara install` sets up on other distributions: the package, the lock
# screen's PAM service, polkit (Mandara is its own authentication agent),
# the desktop portals, fonts and icons, the D-Bus services Studio talks
# to, and optionally Mandara Login on greetd. Compositors stay yours to
# enable; per-user settings live in the Home Manager module.
self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.mandara;
  inherit (lib)
    literalExpression
    mkDefault
    mkEnableOption
    mkIf
    mkMerge
    mkOption
    types
    ;
  # The PAM service `mandara lock` uses (packaging/pam/mandara-lock).
  lockService = "mandara-lock";
  # Mandara Login's state (last user and session) and the folder each
  # session shares its wallpaper into (crates/mandara-greeter/src/install.rs,
  # crates/mandara-config/src/paths.rs).
  greeterStateDir = "/var/lib/mandara-greeter";
  loginWallpaperDir = "/var/lib/mandara-login/wallpapers";
in
{
  options.programs.mandara = {
    enable = mkEnableOption "Mandara Shell, the design-first Wayland desktop shell";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = literalExpression "mandara.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The Mandara package to install.";
    };

    fonts.enable = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Install the fonts Mandara's defaults use: Inter, JetBrains Mono, Noto
        (Arabic and other scripts, CJK) and Noto Color Emoji. The Alexandria
        and Cairo display fonts are Google Fonts; add them with
        `fonts.packages = [ (pkgs.google-fonts.override { fonts = [ "Alexandria" "Cairo" ]; }) ];`.
      '';
    };

    services.enable = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Enable the D-Bus services Studio and the panels talk to:
        AccountsService (Users), GeoClue (location for weather), UDisks2
        (Drives), UPower (battery), power-profiles-daemon (power profiles,
        unless TLP or auto-cpufreq is enabled) and GNOME Keyring (saved
        secrets). Each is a default you can override. Sound (PipeWire),
        NetworkManager and Bluetooth stay your choice: enable them as usual
        for the Network, Bluetooth and sound panels.
      '';
    };

    monitorBrightness.enable = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Let Studio → Displays set external monitors' brightness over DDC/CI:
        loads `i2c-dev` (`hardware.i2c.enable`). Add yourself to the `i2c`
        group: `users.users.<name>.extraGroups = [ "i2c" ];`.
      '';
    };

    greeter = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Use Mandara Login as the login screen: greetd runs `mandara greeter`
          in the cage kiosk compositor and lists the sessions of your
          display-manager configuration (every compositor you enabled).
          Turn off other display managers when you enable it.
        '';
      };

      unlockKeyring = mkOption {
        type = types.bool;
        default = config.services.gnome.gnome-keyring.enable;
        defaultText = literalExpression "config.services.gnome.gnome-keyring.enable";
        description = "Unlock GNOME Keyring with the password you type at Mandara Login.";
      };
    };
  };

  config = mkIf cfg.enable (mkMerge [
    {
      environment.systemPackages = [
        cfg.package
        pkgs.papirus-icon-theme
        pkgs.sound-theme-freedesktop
        pkgs.xdg-utils
      ];

      # `mandara lock`: its own PAM service (password, plus whatever
      # `security.pam` adds, such as fingerprints when fprintd is on).
      security.pam.services.${lockService} = { };

      # Mandara registers its own polkit authentication agent with the
      # session, so no other agent (polkit-gnome, lxqt-policykit, …) needs to
      # run alongside it.
      security.polkit.enable = true;

      # Opening links and files, the file chooser and the system appearance
      # go through the desktop portal; GTK's is the fallback for every
      # desktop. Your compositor's module adds its own (screen sharing).
      xdg.portal = {
        enable = true;
        extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
        config.common.default = mkDefault [ "gtk" ];
      };

      # Studio's desktop entry, icon and Arabic names.
      environment.pathsToLink = [
        "/share/applications"
        "/share/icons"
      ];
    }

    (mkIf cfg.fonts.enable {
      fonts.packages = with pkgs; [
        inter
        jetbrains-mono
        noto-fonts
        noto-fonts-cjk-sans
        noto-fonts-color-emoji
      ];
    })

    (mkIf cfg.services.enable {
      services.accounts-daemon.enable = mkDefault true;
      services.geoclue2.enable = mkDefault true;
      services.udisks2.enable = mkDefault true;
      services.upower.enable = mkDefault true;
      services.power-profiles-daemon.enable = mkDefault (
        !(config.services.tlp.enable || config.services.auto-cpufreq.enable)
      );
      services.gnome.gnome-keyring.enable = mkDefault true;
    })

    (mkIf cfg.monitorBrightness.enable {
      hardware.i2c.enable = true;
    })

    (mkIf cfg.greeter.enable {
      services.greetd = {
        enable = true;
        settings.default_session = {
          # One output (`-m last`), like `mandara greeter install`; the
          # sessions come from the display-manager session data, which is
          # where NixOS puts every enabled compositor's session file.
          command = lib.concatStringsSep " " [
            "${lib.getExe' pkgs.cage "cage"} -s -m last --"
            "${lib.getExe' pkgs.coreutils "env"}"
            "XDG_DATA_DIRS=${config.services.displayManager.sessionData.desktops}/share:/run/current-system/sw/share"
            "${lib.getExe cfg.package} greeter"
          ];
          user = mkDefault "greeter";
        };
      };

      security.pam.services.greetd.enableGnomeKeyring = mkIf cfg.greeter.unlockKeyring true;

      systemd.tmpfiles.settings."10-mandara-login" = {
        ${greeterStateDir}.d = {
          user = config.services.greetd.settings.default_session.user;
          group = config.services.greetd.settings.default_session.user;
          mode = "0750";
        };
        ${loginWallpaperDir}.d = {
          user = "root";
          group = "root";
          mode = "1777";
        };
      };
    })
  ]);
}
