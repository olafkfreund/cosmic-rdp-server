{
  description = "RDP server for the COSMIC Desktop Environment";

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

        cosmic-rdp-server = craneLib.buildPackage (pkgDef // {
          inherit cargoArtifacts;
        });

        settingsPkgDef = pkgDef // {
          buildInputs = nativeBuildDeps ++ guiNativeBuildDeps;
        };

        settingsCargoArtifacts = craneLib.buildDepsOnly settingsPkgDef;

        cosmic-rdp-settings = craneLib.buildPackage (settingsPkgDef // {
          cargoArtifacts = settingsCargoArtifacts;
          cargoExtraArgs = "--package cosmic-rdp-settings";
        });
      in
      {
        checks = {
          inherit cosmic-rdp-server;
        };

        packages = {
          default = cosmic-rdp-server.overrideAttrs (oldAttrs: {
            buildPhase = ''
              just prefix=$out build-release
            '';
            installPhase = ''
              just prefix=$out install
            '';
          });
          cosmic-rdp-server = self.packages.${system}.default;

          cosmic-rdp-settings = cosmic-rdp-settings.overrideAttrs (oldAttrs: {
            buildPhase = ''
              just prefix=$out build-settings-release
            '';
            installPhase = ''
              just prefix=$out install-settings
            '';
          });
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = self.packages.${system}.default;
          };
          cosmic-rdp-settings = flake-utils.lib.mkApp {
            drv = self.packages.${system}.cosmic-rdp-settings;
          };
        };

        devShells.default = pkgs.mkShell {
          packages = buildDeps ++ nativeBuildDeps ++ guiNativeBuildDeps
            ++ runtimeDeps ++ guiRuntimeDeps ++ (with pkgs; [
            rust-analyzer
            clippy
            rustfmt
            cargo-watch
          ]);

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeDeps ++ guiRuntimeDeps);

          shellHook = ''
            echo "cosmic-rdp-server development environment"
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
        cosmic-rdp-server = import ./nix/module.nix;
      };

      overlays.default = final: prev: {
        cosmic-rdp-server = self.packages.${prev.system}.default;
        cosmic-rdp-settings = self.packages.${prev.system}.cosmic-rdp-settings;
      };
    };
}
