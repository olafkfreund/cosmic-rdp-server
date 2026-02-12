{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.cosmic-ext-rdp-broker;
  settingsFormat = pkgs.formats.toml { };

  configFile = settingsFormat.generate "cosmic-ext-rdp-broker.toml" cfg.settings;
in
{
  options.services.cosmic-ext-rdp-broker = {
    enable = mkEnableOption "RDP for COSMIC Broker for multi-user remote desktop";

    package = mkPackageOption pkgs "cosmic-ext-rdp-broker" {
      default = [ "cosmic-ext-rdp-broker" ];
      example = literalExpression ''
        pkgs.cosmic-ext-rdp-broker
      '';
    };

    serverPackage = mkPackageOption pkgs "cosmic-ext-rdp-server" {
      default = [ "cosmic-ext-rdp-server" ];
      description = "The cosmic-ext-rdp-server package used for per-user sessions.";
    };

    settings = mkOption {
      type = types.submodule {
        freeformType = settingsFormat.type;
        options = {
          bind = mkOption {
            type = types.str;
            default = "0.0.0.0:3389";
            description = "Address and port for the broker to listen on.";
          };

          port_range_start = mkOption {
            type = types.int;
            default = 3390;
            description = "Start of the port range for per-user sessions.";
          };

          port_range_end = mkOption {
            type = types.int;
            default = 3489;
            description = "End of the port range for per-user sessions.";
          };

          pam_service = mkOption {
            type = types.str;
            default = "cosmic-ext-rdp";
            description = "PAM service name for authentication.";
          };

          idle_timeout_secs = mkOption {
            type = types.int;
            default = 3600;
            description = "Seconds of idle time before a session is terminated.";
          };

          max_sessions = mkOption {
            type = types.int;
            default = 100;
            description = "Maximum number of concurrent user sessions.";
          };

          session_policy = mkOption {
            type = types.enum [ "OnePerUser" "ReplaceExisting" ];
            default = "OnePerUser";
            description = "Policy for handling existing sessions when a user reconnects.";
          };

          state_file = mkOption {
            type = types.str;
            default = "/var/lib/cosmic-ext-rdp-broker/sessions.json";
            description = "Path to the persisted session state file.";
          };
        };
      };
      default = { };
      description = "Configuration for the RDP for COSMIC Broker.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Whether to open the RDP port (3389) in the firewall.";
    };
  };

  config = mkIf cfg.enable {
    # Inject server_binary path from the package.
    services.cosmic-ext-rdp-broker.settings.server_binary =
      mkDefault "${cfg.serverPackage}/bin/cosmic-ext-rdp-server";

    # PAM configuration for the cosmic-ext-rdp service.
    security.pam.services.cosmic-ext-rdp = {
      text = ''
        auth    required pam_unix.so
        account required pam_unix.so
      '';
    };

    # System service (runs as root for systemd-run and PAM).
    systemd.services.cosmic-ext-rdp-broker = {
      description = "RDP for COSMIC Session Broker";
      after = [ "network.target" "multi-user.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/cosmic-ext-rdp-broker --config ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;

        # State directory for sessions.json
        StateDirectory = "cosmic-ext-rdp-broker";

        # Security hardening (limited because broker needs root for
        # systemd-run and PAM).
        NoNewPrivileges = false; # Needs to spawn user processes.
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;

        # The broker needs write access to the state directory.
        ReadWritePaths = [
          "/var/lib/cosmic-ext-rdp-broker"
        ];
      };
    };

    # systemd slice for per-user RDP sessions.
    systemd.slices.cosmic-ext-rdp-sessions = {
      description = "RDP for COSMIC User Sessions";
      sliceConfig = {
        # Resource limits for all RDP sessions combined.
        MemoryMax = "8G";
        TasksMax = 4096;
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
