{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.cosmic-rdp-server;
  settingsFormat = pkgs.formats.toml { };
  configFile = settingsFormat.generate "cosmic-rdp-server.toml" cfg.settings;
in
{
  options.services.cosmic-rdp-server = {
    enable = mkEnableOption "COSMIC RDP Server for remote desktop access";

    package = mkPackageOption pkgs "cosmic-rdp-server" {
      default = [ "cosmic-rdp-server" ];
      example = literalExpression ''
        pkgs.cosmic-rdp-server.override { }
      '';
    };

    settings = mkOption {
      type = types.submodule {
        freeformType = settingsFormat.type;
        options = {
          bind = mkOption {
            type = types.str;
            default = "0.0.0.0:3389";
            description = "Address and port to bind the RDP server to.";
          };

          capture = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                fps = mkOption {
                  type = types.int;
                  default = 30;
                  description = "Target frames per second for screen capture.";
                };
              };
            };
            default = { };
            description = "Screen capture settings.";
          };

          encode = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                encoder = mkOption {
                  type = types.enum [ "auto" "vaapi" "nvenc" "software" ];
                  default = "auto";
                  description = "Preferred video encoder backend.";
                };
                preset = mkOption {
                  type = types.str;
                  default = "ultrafast";
                  description = "H.264 encoding preset.";
                };
              };
            };
            default = { };
            description = "Video encoding settings.";
          };
        };
      };
      default = { };
      description = ''
        Configuration for the COSMIC RDP Server.

        Settings are written to a TOML configuration file.
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Whether to open the RDP port in the firewall.";
    };
  };

  config = mkIf cfg.enable {
    systemd.user.services.cosmic-rdp-server = {
      description = "COSMIC RDP Server";
      after = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/cosmic-rdp-server --config ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;

        # Security hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
      };
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [
      (
        let
          parts = splitString ":" cfg.settings.bind;
          port = last parts;
        in
        toInt port
      )
    ];
  };
}
