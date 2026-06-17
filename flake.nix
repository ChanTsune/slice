{
  inputs = {
    naersk.url = "github:nix-community/naersk/master";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, utils, naersk }:
    utils.lib.eachDefaultSystem (system:
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
          meta = with pkgs.lib; {
            description = "Slice file contents using Python-like slice notation";
            homepage = "https://github.com/ChanTsune/slice";
            license = with licenses; [ asl20 mit ];
            mainProgram = "slice";
          };
        };
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [ cargo rustc rustfmt rustPackages.clippy ];
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      });
}
