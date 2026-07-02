#!/bin/sh
# Install the latest (or a pinned) `slice` release binary on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/ChanTsune/slice/main/install.sh | sh
#
# Environment overrides:
#   SLICE_VERSION       version/tag to install (default: latest release)
#   SLICE_INSTALL_DIR   directory to install into (default: $HOME/.local/bin)
set -eu

REPO="ChanTsune/slice"
BIN="slice"
INSTALL_DIR="${SLICE_INSTALL_DIR:-$HOME/.local/bin}"

err() {
	printf 'error: %s\n' "$1" >&2
	exit 1
}

have() {
	command -v "$1" >/dev/null 2>&1
}

# Map the host to one of the published release target triples. Linux ships musl
# builds (and a gnueabihf build for 32-bit arm); macOS and FreeBSD ship native
# builds.
detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$os" in
	Linux)
		case "$arch" in
		x86_64 | amd64) echo "x86_64-unknown-linux-musl" ;;
		aarch64 | arm64) echo "aarch64-unknown-linux-musl" ;;
		riscv64) echo "riscv64gc-unknown-linux-musl" ;;
		armv7l | armv6l | arm) echo "arm-unknown-linux-gnueabihf" ;;
		*) err "unsupported Linux architecture: $arch" ;;
		esac
		;;
	Darwin)
		case "$arch" in
		x86_64) echo "x86_64-apple-darwin" ;;
		arm64 | aarch64) echo "aarch64-apple-darwin" ;;
		*) err "unsupported macOS architecture: $arch" ;;
		esac
		;;
	FreeBSD)
		case "$arch" in
		x86_64 | amd64) echo "x86_64-unknown-freebsd" ;;
		*) err "unsupported FreeBSD architecture: $arch" ;;
		esac
		;;
	*)
		err "unsupported OS: $os (Windows users: see install.ps1 or winget/cargo)"
		;;
	esac
}

# Resolve the latest tag by following the /releases/latest redirect, which
# avoids the GitHub API rate limit.
latest_version() {
	url="https://github.com/$REPO/releases/latest"
	if have curl; then
		effective=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$url") ||
			err "could not reach GitHub to resolve the latest version"
	elif have wget; then
		effective=$(wget --max-redirect=5 -S -O /dev/null "$url" 2>&1 |
			awk '/^[[:space:]]*Location:/ { u = $2 } END { print u }')
	else
		err "neither curl nor wget is available"
	fi
	# The redirect lands on .../releases/tag/<version>. Anything else (e.g. a
	# repo with no releases redirects to .../releases, and a swallowed wget
	# failure leaves this empty) is not a version.
	case "${effective:-}" in
	*/releases/tag/*) ;;
	*) err "no published release found for $REPO (resolved to: ${effective:-<empty>})" ;;
	esac
	printf '%s' "${effective##*/}"
}

download() {
	if have curl; then
		curl -fsSL "$1" -o "$2" || err "download failed: $1"
	elif have wget; then
		wget -qO "$2" "$1" || err "download failed: $1"
	else
		err "neither curl nor wget is available"
	fi
}

main() {
	have tar || err "tar is required"

	target=$(detect_target)
	version="${SLICE_VERSION:-$(latest_version)}"
	[ -n "$version" ] || err "could not determine a version to install"

	stem="$BIN-$version-$target"
	archive="$stem.tar.gz"
	url="https://github.com/$REPO/releases/download/$version/$archive"

	printf 'Installing %s %s (%s)\n' "$BIN" "$version" "$target"

	tmp=$(mktemp -d 2>/dev/null || mktemp -d -t slice)
	trap 'rm -rf "$tmp"' EXIT

	download "$url" "$tmp/$archive"
	tar -xzf "$tmp/$archive" -C "$tmp" ||
		err "could not extract $archive"

	# Archives expand into a <stem>/ directory containing the binary.
	src="$tmp/$stem/$BIN"
	[ -f "$src" ] || err "binary not found in archive (expected $stem/$BIN)"

	mkdir -p "$INSTALL_DIR"
	install -m 0755 "$src" "$INSTALL_DIR/$BIN" 2>/dev/null ||
		{ cp "$src" "$INSTALL_DIR/$BIN" && chmod 0755 "$INSTALL_DIR/$BIN"; } ||
		err "could not install to $INSTALL_DIR (set SLICE_INSTALL_DIR to a writable directory)"

	printf 'Installed %s to %s\n' "$BIN" "$INSTALL_DIR/$BIN"

	# $PATH must print literally as part of the suggested command, not expand.
	# shellcheck disable=SC2016
	case ":$PATH:" in
	*":$INSTALL_DIR:"*) : ;;
	*) printf '\nNote: %s is not on your PATH. Add it, e.g.:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR" ;;
	esac

	printf '\nShell completions and a man page can be generated with:\n  %s --generate complete-bash   # or complete-zsh, complete-fish, man\n' "$BIN"
}

main
