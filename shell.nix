{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    rustc
    cargo
    rustfmt
    clippy
  ];

  buildInputs = with pkgs; [
    fontconfig
    freetype
    libxkbcommon
    wayland
    libGL
    vulkan-loader
    
    # X11 / Windowing dependencies
    libX11
    libXcursor
    libXrandr
    libXi
    libXinerama
  ];

  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
      pkgs.fontconfig
      pkgs.freetype
      pkgs.libxkbcommon
      pkgs.wayland
      pkgs.libGL
      pkgs.vulkan-loader
      pkgs.libX11
      pkgs.libXcursor
      pkgs.libXrandr
      pkgs.libXi
      pkgs.libXinerama
    ]}:$LD_LIBRARY_PATH"

    echo "Rust dev environment loaded for MRKv"
    echo "Rust:  $(rustc --version)"
    echo "Cargo: $(cargo --version)"
  '';
}
