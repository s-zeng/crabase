{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    systems.url = "github:nix-systems/default";

    # Dev tools
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import inputs.systems;
      imports = [
        inputs.treefmt-nix.flakeModule
      ];
      perSystem = { config, self', pkgs, lib, system, ... }:
        let
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          nonRustDeps = [
            pkgs.libiconv
            pkgs.pkg-config
            pkgs.openssl
            pkgs.pandoc
            pkgs.texlive.combined.scheme-small
          ];

          # Python polars must be ABI-matched to the Rust polars (0.46) we
          # link against — newer Python polars versions can't deserialise the
          # LazyFrame plans pyo3-polars emits ("dsl magic bytes not found").
          # nixpkgs ships a much newer polars, so we pin via the prebuilt PyPI
          # wheel for each supported platform.
          polarsWheelInfo = {
            "x86_64-darwin" = { url = "https://files.pythonhosted.org/packages/75/59/ff6185f1cd3898a655fb69571986433adec80c5fe5dbd37edf1025dbbd17/polars-1.22.0-cp39-abi3-macosx_10_12_x86_64.whl"; hash = "sha256-YlD4OLkW+rI8yv6Qko15Uq/DKNMWyVa0LRUrIMhv/Zw="; };
            "aarch64-darwin" = { url = "https://files.pythonhosted.org/packages/4e/89/ac9178aaf4bfce1087d311ddf540b259a517b584a62ba75dfd114a38e049/polars-1.22.0-cp39-abi3-macosx_11_0_arm64.whl"; hash = "sha256-XuPPN4MgVwnOMfBw8rTuQpb+wI8sdEqcN6zH02ASECI="; };
            "x86_64-linux" = { url = "https://files.pythonhosted.org/packages/91/b6/d7967ca14b8bacf7d3db96b55213571c43443f8802229606cca60458780b/polars-1.22.0-cp39-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl"; hash = "sha256-lPJbTvEx2gRtBbgjXF8pmXYw7iEl68BVO5Iljoj3qPo="; };
            "aarch64-linux" = { url = "https://files.pythonhosted.org/packages/a2/ae/c014bcb259757acd9081b4db76d19157c8978cb6eeb864f80ac0544e85cd/polars-1.22.0-cp39-abi3-manylinux_2_24_aarch64.whl"; hash = "sha256-cp5r6KiEgSogZRgZWi+0B7YZYjI4hgle3hoqk0zbFBA="; };
          };

          # Inject pinned polars + crabase into a python package set so users
          # can compose them like any other python deps:
          #   crabase.packages.${system}.python.withPackages (ps: [ ps.crabase ])
          # Polars is built from a prebuilt wheel (not via overridePythonAttrs)
          # because the upstream derivation runs maturin even when format=wheel.
          pythonForCrabase = pkgs.python3.override {
            self = pythonForCrabase;
            packageOverrides = pyfinal: pyprev: {
              polars = pyfinal.buildPythonPackage {
                pname = "polars";
                version = "1.22.0";
                format = "wheel";
                src = pkgs.fetchurl polarsWheelInfo.${system};
                doCheck = false;
              };

              crabase = pyfinal.buildPythonPackage {
                pname = "crabase";
                version = "0.1.0";
                pyproject = true;

                # Whole repo is the source because crates/crabase-py has a path
                # dependency on the root crate (`crabase = { path = "../.." }`).
                src = ./.;
                postUnpack = ''
                  sourceRoot=$sourceRoot/crates/crabase-py
                '';

                cargoDeps = pkgs.rustPlatform.importCargoLock {
                  lockFile = ./crates/crabase-py/Cargo.lock;
                };

                nativeBuildInputs = with pkgs.rustPlatform; [
                  cargoSetupHook
                  maturinBuildHook
                ];

                propagatedBuildInputs = [ pyfinal.polars ];

                pythonImportsCheck = [ "crabase" ];
              };
            };
          };
        in
        {
          # Rust package
          packages.default = pkgs.rustPlatform.buildRustPackage {
            inherit (cargoToml.package) name version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = nonRustDeps;
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
            LIBRARY_PATH = "${pkgs.libiconv}/lib";
          };

          # Python interpreter with `crabase` (and a polars version matched to
          # the linked Rust polars 0.46) available in its package set.
          # Compose freely:
          #   python.withPackages (ps: [ ps.crabase ps.numpy ])
          packages.python = pythonForCrabase;

          # The bare crabase python package, for adding to systemPackages or
          # another python.withPackages call.
          packages.crabase-py = pythonForCrabase.pkgs.crabase;

          # Convenience env: a Python interpreter that already has `crabase`
          # importable. Use via `nix shell .#python-env` then `python3 -c ...`.
          packages.python-env = pythonForCrabase.withPackages (ps: [ ps.crabase ]);

          # Rust dev environment
          devShells.default = pkgs.mkShell {
            inputsFrom = [
              config.treefmt.build.devShell
            ];
            shellHook = ''
              # For rust-analyzer 'hover' tooltips to work.
              export RUST_SRC_PATH=${pkgs.rustPlatform.rustLibSrc}
              # openssl
              export OPENSSL_DIR="${pkgs.openssl.dev}"
              export OPENSSL_LIB_DIR="${pkgs.openssl.out}/lib"
              export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig"
              export OPENSSL_DEV="${pkgs.openssl.dev}"
              # libiconv
              export LIBRARY_PATH="${pkgs.libiconv}/lib:$LIBRARY_PATH"
            '';
            buildInputs = nonRustDeps;
            nativeBuildInputs = with pkgs; [
              just
              rustc
              cargo
              cargo-watch
              cargo-insta
              clippy
              rust-analyzer
              python312
              maturin
            ];
          };

          # Add your auto-formatters here.
          # cf. https://numtide.github.io/treefmt/
          treefmt.config = {
            projectRootFile = "flake.nix";
            programs = {
              nixpkgs-fmt.enable = true;
              rustfmt.enable = true;
              ruff-format.enable = true;
            };
          };
        };
    };
}
