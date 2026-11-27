{
  description = "Patina: an identity-driven, event-sourced personal task system";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = lib.genAttrs systems;
      toolchainSpec = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain;
      androidRustTargets = [
        "aarch64-linux-android"
        "x86_64-linux-android"
      ];

      pkgsFor =
        system: config:
        import nixpkgs {
          inherit system config;
          overlays = [ rust-overlay.overlays.default ];
        };

      mkDevShell =
        {
          pkgs,
          rustTargets ? [ ],
          extraPackages ? [ ],
          env ? { },
          shellHook ? "",
        }:
        let
          # Android std targets are added here, not in rust-toolchain.toml, so
          # rustup users on Windows/macOS do not download them by default.
          rustToolchain = pkgs.rust-bin.fromRustupToolchain (
            toolchainSpec // { targets = (toolchainSpec.targets or [ ]) ++ rustTargets; }
          );
          # nixpkgs lags Slint; drop this override once pkgs.slint-lsp >= the slint crate version.
          slint-lsp = pkgs.slint-lsp.overrideAttrs (
            finalAttrs: _: {
              version = "1.19.0";
              src = pkgs.fetchCrate {
                pname = "slint-lsp";
                inherit (finalAttrs) version;
                hash = "sha256-6KOlnDcaurRsXdQmgcyjLPyDMFpe+D1CgJPyFtiKb78=";
              };
              cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
                inherit (finalAttrs) src;
                name = "slint-lsp-${finalAttrs.version}-vendor";
                hash = "sha256-x+DrL82HT6r2uubapJE3Yig6tcgRexzI0HtNI3az++w=";
              };
            }
          );
          # nixpkgs lags Slint; drop this override once pkgs.slint-tr-extractor >= the slint crate version.
          slint-tr-extractor = pkgs.slint-tr-extractor.overrideAttrs (
            finalAttrs: _: {
              version = "1.19.1";
              src = pkgs.fetchCrate {
                pname = "slint-tr-extractor";
                inherit (finalAttrs) version;
                hash = "sha256-rDpImUAhDEZdXHALL2rMkYKYxeqTHkqxKUUybxtAA8I=";
              };
              cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
                inherit (finalAttrs) src;
                name = "slint-tr-extractor-${finalAttrs.version}-vendor";
                hash = "sha256-UPJpEWwVoO1C0uOnoOvpXvcd8TtLsqFJTUK5qLDfUFg=";
              };
            }
          );
          # Libraries winit/femtovg/fontique dlopen at run time on Linux.
          runtimeLibs = lib.optionals pkgs.stdenv.hostPlatform.isLinux (
            with pkgs;
            [
              fontconfig
              freetype
              libGL
              libxkbcommon
              wayland
              libx11
              libxcursor
              libxi
              libxrandr
            ]
          );
        in
        pkgs.mkShell (
          {
            packages = [
              rustToolchain
              slint-lsp
              slint-tr-extractor
            ]
            ++ (with pkgs; [
              just
              gettext
              nixfmt
              pkg-config
              curl
              unzip
            ])
            ++ extraPackages;
            buildInputs = runtimeLibs;
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            shellHook = ''
              echo "patina dev shell: $(rustc --version)"
              echo "run 'just' to list tasks"
            ''
            + shellHook;
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
          }
          // env
        );
    in
    {
      devShells = forAllSystems (
        system:
        {
          default = mkDevShell { pkgs = pkgsFor system { }; };
        }
        // lib.optionalAttrs (system == "x86_64-linux") {
          android =
            let
              pkgs = pkgsFor system {
                # The Android SDK is unfree.
                allowUnfree = true;
                android_sdk.accept_license = true;
              };
              androidSdk =
                (pkgs.androidenv.composeAndroidPackages {
                  platformVersions = [ "35" ];
                  buildToolsVersions = [ "35.0.0" ];
                  includeNDK = true;
                  ndkVersions = [ "latest" ];
                  includeEmulator = false;
                  includeSystemImages = false;
                }).androidsdk;
              sdkRoot = "${androidSdk}/libexec/android-sdk";
            in
            mkDevShell {
              inherit pkgs;
              rustTargets = androidRustTargets;
              extraPackages = [
                androidSdk
                pkgs.jdk17
                pkgs.cargo-apk
              ];
              env = {
                ANDROID_HOME = sdkRoot;
                ANDROID_NDK_ROOT = "${sdkRoot}/ndk-bundle";
                JAVA_HOME = pkgs.jdk17.home;
              };
              shellHook = ''
                export PATH="${sdkRoot}/platform-tools:$PATH"
              '';
            };
        }
      );

      checks = forAllSystems (system: {
        dev-shell = self.devShells.${system}.default;
      });
    };
}
