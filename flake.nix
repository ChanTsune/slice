{
  inputs = {
    naersk.url = "github:nix-community/naersk/master";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      naersk,
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        naersk-lib = pkgs.callPackage naersk { };
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        packages.default = naersk-lib.buildPackage {
          name = "slice";
          version = cargoToml.package.version;
          src = ./.;

          nativeBuildInputs = [ pkgs.installShellFiles ];

          # Ship the bash/zsh/fish completions and man page that the release
          # tarball provides. naersk copies the built binary into $out/bin
          # before running postInstall, so it can generate them here.
          postInstall = ''
            installShellCompletion --cmd slice \
              --bash <($out/bin/slice --generate complete-bash) \
              --zsh  <($out/bin/slice --generate complete-zsh) \
              --fish <($out/bin/slice --generate complete-fish)

            $out/bin/slice --generate man > slice.1
            installManPage slice.1
          '';

          meta = with pkgs.lib; {
            description = "Slice file contents using Python-like slice notation";
            homepage = "https://github.com/ChanTsune/slice";
            license = with licenses; [
              asl20
              mit
            ];
            mainProgram = "slice";
          };
        };
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            rustPackages.clippy
          ];
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };

        # `nix fmt` — nixfmt-tree is a zero-config treefmt wrapper that
        # recursively formats every *.nix file with nixfmt (RFC style).
        formatter = pkgs.nixfmt-tree;
      }
    );
}
