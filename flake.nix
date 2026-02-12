{
  description = "RDP server for the COSMIC desktop environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    nix-filter.url = "github:numtide/nix-filter";
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = { self, nixpkgs, flake-utils, nix-filter, crane }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        runtimeDeps = with pkgs; [
          wayland
          libxkbcommon
          pipewire
          gst_all_1.gstreamer
          gst_all_1.gst-plugins-base
          gst_all_1.gst-plugins-good
          gst_all_1.gst-plugins-bad
          gst_all_1.gst-plugins-ugly
          gst_all_1.gst-vaapi
          libei
        ];

        # Additional runtime deps for the settings GUI (libcosmic).
        guiRuntimeDeps = with pkgs; [
          expat
          fontconfig
          freetype
          libGL
          mesa
          vulkan-loader
          dbus
        ];

        buildDeps = with pkgs; [
          pkg-config
          just
          rustPlatform.bindgenHook
        ];

        nativeBuildDeps = with pkgs; [
          wayland
          libxkbcommon
          pipewire
          gst_all_1.gstreamer
          gst_all_1.gst-plugins-base
          libei
          openssl
          clang
        ];

        guiNativeBuildDeps = with pkgs; [
          expat
          fontconfig
          freetype
          libGL
          mesa
          vulkan-loader
          dbus
        ];

        pkgDef = {
          src = nix-filter.lib.filter {
            root = ./.;
            exclude = [
              ".git"
              "nix"
              "flake.nix"
              "flake.lock"
              "README.md"
              "LICENSE"
              "CLAUDE.md"
              ".gitignore"
            ];
          };

          strictDeps = true;
          nativeBuildInputs = buildDeps;
          buildInputs = nativeBuildDeps;
        };

        cargoArtifacts = craneLib.buildDepsOnly pkgDef;

        cosmic-ext-rdp-server = craneLib.buildPackage (pkgDef // {
          inherit cargoArtifacts;
        });

        settingsPkgDef = pkgDef // {
          buildInputs = nativeBuildDeps ++ guiNativeBuildDeps;
        };

        settingsCargoArtifacts = craneLib.buildDepsOnly settingsPkgDef;

        cosmic-ext-rdp-settings = craneLib.buildPackage (settingsPkgDef // {
          cargoArtifacts = settingsCargoArtifacts;
          cargoExtraArgs = "--package cosmic-ext-rdp-settings";
        });

        # Broker only needs base deps (no GUI, no GStreamer runtime).
        brokerPkgDef = pkgDef // {
          buildInputs = with pkgs; [
            wayland
            libxkbcommon
            pipewire
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            libei
            openssl
            clang
            linux-pam
          ];
        };

        brokerCargoArtifacts = craneLib.buildDepsOnly brokerPkgDef;

        cosmic-ext-rdp-broker = craneLib.buildPackage (brokerPkgDef // {
          cargoArtifacts = brokerCargoArtifacts;
          cargoExtraArgs = "--package cosmic-ext-rdp-broker";
        });
      in
      {
        checks = {
          inherit cosmic-ext-rdp-server;
        };

        packages = {
          default = cosmic-ext-rdp-server.overrideAttrs (oldAttrs: {
            buildPhase = ''
              just prefix=$out build-release
            '';
            installPhase = ''
              just prefix=$out install
            '';
          });
          cosmic-ext-rdp-server = self.packages.${system}.default;

          cosmic-ext-rdp-settings = cosmic-ext-rdp-settings.overrideAttrs (oldAttrs: {
            buildPhase = ''
              just prefix=$out build-settings-release
            '';
            installPhase = ''
              just prefix=$out install-settings
            '';
          });

          cosmic-ext-rdp-broker = cosmic-ext-rdp-broker.overrideAttrs (oldAttrs: {
            buildPhase = ''
              just prefix=$out build-broker-release
            '';
            installPhase = ''
              just prefix=$out install-broker
            '';
          });
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = self.packages.${system}.default;
          };
          cosmic-ext-rdp-settings = flake-utils.lib.mkApp {
            drv = self.packages.${system}.cosmic-ext-rdp-settings;
          };
          cosmic-ext-rdp-broker = flake-utils.lib.mkApp {
            drv = self.packages.${system}.cosmic-ext-rdp-broker;
          };
        };

        devShells.default = pkgs.mkShell {
          packages = buildDeps ++ nativeBuildDeps ++ guiNativeBuildDeps
            ++ runtimeDeps ++ guiRuntimeDeps ++ (with pkgs; [
            rust-analyzer
            clippy
            rustfmt
            cargo-watch
            linux-pam
          ]);

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeDeps ++ guiRuntimeDeps);

          shellHook = ''
            echo "cosmic-ext-rdp-server development environment"
            echo "  just build-debug   - Debug build"
            echo "  just build-release - Release build"
            echo "  just check         - Clippy with pedantic"
            echo "  just run           - Run with backtrace"
          '';
        };
      }
    ) // {
      nixosModules = {
        default = import ./nix/module.nix;
        cosmic-ext-rdp-server = import ./nix/module.nix;
        cosmic-ext-rdp-broker = import ./nix/broker-module.nix;
      };

      homeManagerModules = {
        default = import ./nix/home-manager.nix;
        cosmic-ext-rdp-server = import ./nix/home-manager.nix;
      };

      overlays.default = final: prev: {
        cosmic-ext-rdp-server = self.packages.${prev.system}.default;
        cosmic-ext-rdp-settings = self.packages.${prev.system}.cosmic-ext-rdp-settings;
        cosmic-ext-rdp-broker = self.packages.${prev.system}.cosmic-ext-rdp-broker;
      };
    };
}
