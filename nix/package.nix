{ lib
, rustPlatform
, pkg-config
, just
, wayland
, libxkbcommon
, pipewire
, gst_all_1
, libei
, openssl
}:

rustPlatform.buildRustPackage {
  pname = "cosmic-ext-rdp-server";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.difference ../. (
      lib.fileset.unions [
        ../nix
        ../flake.nix
        ../flake.lock
        ../README.md
        ../LICENSE
        ../CLAUDE.md
        ../.gitignore
      ]
    );
  };

  cargoLock.lockFile = ../Cargo.lock;

  strictDeps = true;

  nativeBuildInputs = [
    pkg-config
    just
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    wayland
    libxkbcommon
    pipewire
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    libei
    openssl
  ];

  buildPhase = ''
    runHook preBuild
    just prefix=$out build-release
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    just prefix=$out install
    runHook postInstall
  '';

  meta = with lib; {
    description = "RDP server for the COSMIC desktop environment";
    homepage = "https://github.com/olafkfreund/cosmic-ext-rdp-server";
    license = licenses.gpl3Only;
    maintainers = [ ];
    platforms = platforms.linux;
    mainProgram = "cosmic-ext-rdp-server";
  };
}
