{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.cosmic-rdp-server;
  settingsFormat = pkgs.formats.toml { };

  effectiveSettings = cfg.settings // optionalAttrs cfg.auth.enable {
    auth = {
      enable = true;
      username = cfg.auth.username;
      domain = cfg.auth.domain;
    };
  };

  configFile = settingsFormat.generate "cosmic-rdp-server.toml" effectiveSettings;

  startScript = pkgs.writeShellScript "cosmic-rdp-server-start" ''
    CONFIG="${configFile}"

    if [ -n "''${CREDENTIALS_DIRECTORY:-}" ] && [ -f "$CREDENTIALS_DIRECTORY/rdp-password" ]; then
      RUNTIME_CONFIG="''${XDG_RUNTIME_DIR}/cosmic-rdp-server/config.toml"
      ${pkgs.coreutils}/bin/mkdir -p "$(${pkgs.coreutils}/bin/dirname "$RUNTIME_CONFIG")"
      ${pkgs.coreutils}/bin/cp "$CONFIG" "$RUNTIME_CONFIG"
      PASSWORD=$(${pkgs.coreutils}/bin/cat "$CREDENTIALS_DIRECTORY/rdp-password")
      ${pkgs.coreutils}/bin/cat >> "$RUNTIME_CONFIG" <<TOMLEOF

[auth]
password = "$PASSWORD"
TOMLEOF
      CONFIG="$RUNTIME_CONFIG"
    fi

    exec ${cfg.package}/bin/cosmic-rdp-server --config "$CONFIG"
  '';
in
{
  options.services.cosmic-rdp-server = {
    enable = mkEnableOption "COSMIC RDP Server for remote desktop access (user-level)";

    package = mkPackageOption pkgs "cosmic-rdp-server" {
      default = [ "cosmic-rdp-server" ];
      example = literalExpression ''
        pkgs.cosmic-rdp-server
      '';
    };

    installSettings = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to install the COSMIC RDP Settings GUI application.";
    };

    settingsPackage = mkPackageOption pkgs "cosmic-rdp-settings" {
      default = [ "cosmic-rdp-settings" ];
      example = literalExpression ''
        pkgs.cosmic-rdp-settings
      '';
    };

    autoStart = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Whether to start the RDP server automatically with the graphical session.

        When disabled, you can start it manually with:
          systemctl --user start cosmic-rdp-server
      '';
    };

    auth = {
      enable = mkEnableOption "NLA (Network Level Authentication) via CredSSP";

      username = mkOption {
        type = types.str;
        default = "";
        description = "Username for NLA authentication.";
      };

      domain = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Windows domain for NLA authentication (optional).";
      };

      passwordFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to a file containing the NLA password.

          The file should contain only the password (no trailing newline).
          Loaded via systemd LoadCredential so it never appears in the
          Nix store. Compatible with agenix/sops-nix secrets.
        '';
        example = "/run/agenix/cosmic-rdp-password";
      };
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
                channel_capacity = mkOption {
                  type = types.int;
                  default = 4;
                  description = "PipeWire channel capacity (number of buffered frames).";
                };
                multi_monitor = mkOption {
                  type = types.bool;
                  default = false;
                  description = "Enable multi-monitor capture.";
                };
              };
            };
            default = { };
            description = "Screen capture settings.";
          };

          audio = mkOption {
            type = types.submodule {
              freeformType = settingsFormat.type;
              options = {
                enable = mkOption {
                  type = types.bool;
                  default = true;
                  description = "Enable RDPSND audio forwarding.";
                };
                sample_rate = mkOption {
                  type = types.int;
                  default = 44100;
                  description = "Audio sample rate in Hz.";
                };
                channels = mkOption {
                  type = types.int;
                  default = 2;
                  description = "Number of audio channels (1 = mono, 2 = stereo).";
                };
              };
            };
            default = { };
            description = "Audio forwarding settings (RDPSND).";
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
                bitrate = mkOption {
                  type = types.int;
                  default = 10000000;
                  description = "Target bitrate in bits per second.";
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
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ]
      ++ optional cfg.installSettings cfg.settingsPackage;

    assertions = [
      {
        assertion = cfg.auth.enable -> cfg.auth.username != "";
        message = "services.cosmic-rdp-server.auth.username must be set when auth is enabled.";
      }
      {
        assertion = cfg.auth.enable -> cfg.auth.passwordFile != null;
        message = "services.cosmic-rdp-server.auth.passwordFile must be set when auth is enabled.";
      }
    ];

    systemd.user.services.cosmic-rdp-server = {
      Unit = {
        Description = "COSMIC RDP Server";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };

      Service = {
        Type = "simple";
        ExecStart = toString startScript;
        Restart = "on-failure";
        RestartSec = 5;
        RuntimeDirectory = "cosmic-rdp-server";

        LoadCredential = optional (cfg.auth.enable && cfg.auth.passwordFile != null)
          "rdp-password:${cfg.auth.passwordFile}";

        # Security hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        RestrictSUIDSGID = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
      };

      Install = mkIf cfg.autoStart {
        WantedBy = [ "graphical-session.target" ];
      };
    };
  };
}
