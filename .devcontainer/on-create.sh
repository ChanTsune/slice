#!/usr/bin/env bash
# Installs direnv + nix-direnv and pre-builds the flake dev shell so the
# first terminal already has the Rust toolchain on PATH.
set -euxo pipefail

# Nix is installed multi-user; its profile script is only sourced by login
# shells, so source it explicitly here.
. /etc/profile.d/nix.sh

# The direnv volume is created root-owned on first mount.
sudo chown "$(id -un):$(id -gn)" "$HOME/.local/share/direnv"

nix profile add nixpkgs#direnv nixpkgs#nix-direnv nixpkgs#gh

# nix-direnv caches the dev shell and protects it from garbage collection.
mkdir -p "$HOME/.config/direnv"
echo 'source $HOME/.nix-profile/share/nix-direnv/direnvrc' > "$HOME/.config/direnv/direnvrc"

echo 'eval "$(direnv hook bash)"' >> "$HOME/.bashrc"
echo 'eval "$(direnv hook zsh)"' >> "$HOME/.zshrc"

# Trust the repository's .envrc and build the dev shell up front.
direnv allow .
direnv exec . cargo --version
