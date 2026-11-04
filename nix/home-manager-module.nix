# Home Manager module: Mandara for one user. Installs the package, can
# manage ~/.config/mandara/config.toml from Nix, and can start the shell as
# a systemd user service. System-wide pieces (PAM, polkit, portals, fonts,
# Mandara Login) are in the NixOS module.
self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.mandara;
  toml = pkgs.formats.toml { };
  inherit (lib)
    literalExpression
    mkEnableOption
    mkIf
    mkMerge
    mkOption
    types
    ;
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

    settings = mkOption {
      inherit (toml) type;
      default = { };
      example = literalExpression ''
        {
          bar.height = 30;
          typography.ui_font = "Inter";
        }
      '';
      description = ''
        Mandara's settings, written to {file}`~/.config/mandara/config.toml`
        (see docs/configuration.md; keys you leave out keep their defaults).

        Mandara Studio edits that file live. While it is left empty (the
        default) Home Manager doesn't touch the file and Studio's changes
        persist. Once you set anything here the file belongs to Home Manager:
        Studio's changes still preview live but are not saved, and the next
        `home-manager switch` puts your Nix settings back.
      '';
    };

    systemd = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Start Mandara as a systemd user service with your graphical
          session, instead of from your compositor's autostart. Your
          compositor must start {option}`programs.mandara.systemd.target`
          and pass `WAYLAND_DISPLAY` and its IPC socket variables to the
          systemd user manager (Niri does, and so does Hyprland's Home
          Manager module with its systemd integration on). Don't let
          `mandara setup --autostart` start it a second time.
        '';
      };

      target = mkOption {
        type = types.str;
        default = "graphical-session.target";
        description = "The systemd user target that starts and stops Mandara.";
      };
    };
  };

  config = mkIf cfg.enable (mkMerge [
    {
      home.packages = [ cfg.package ];
    }

    (mkIf (cfg.settings != { }) {
      xdg.configFile."mandara/config.toml".source = toml.generate "mandara-config.toml" cfg.settings;
    })

    (mkIf cfg.systemd.enable {
      systemd.user.services.mandara = {
        Unit = {
          Description = "Mandara Shell";
          Documentation = "https://github.com/theHamdiz/mandara";
          PartOf = [ cfg.systemd.target ];
          After = [ cfg.systemd.target ];
          ConditionEnvironment = "WAYLAND_DISPLAY";
        };
        Service = {
          # `mandara` is the shell's own restart supervisor; systemd only
          # steps in when the supervisor itself stops.
          ExecStart = lib.getExe cfg.package;
          Restart = "on-failure";
          KillMode = "mixed";
        };
        Install.WantedBy = [ cfg.systemd.target ];
      };
    })
  ]);
}
