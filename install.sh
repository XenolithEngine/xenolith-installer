#!/bin/sh
# Xenolith Installer CLI — one-line installer.
#
#   curl -fsSL https://raw.githubusercontent.com/XenolithEngine/xenolith-installer/main/install.sh | sh
#
# Downloads the CLI for your platform from the latest GitHub release, clears the
# macOS quarantine flag, and drops it on your PATH. Then:
#   xenolith-installer-cli install   # provision the SDK for this machine
#   xenolith-installer-cli new myapp
#   xenolith-installer-cli build myapp --run
#
# Override the install dir with XENOLITH_BIN=/some/dir.
set -eu

REPO="XenolithEngine/xenolith-installer"
BIN="xenolith-installer-cli"
DEST="${XENOLITH_BIN:-$HOME/.local/bin}"

say()  { printf '%s\n' "$*"; }
err()  { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- pick a downloader ---------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
	dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
	dl() { wget -qO "$2" "$1"; }
else
	err "need curl or wget to download"
fi
command -v tar >/dev/null 2>&1 || err "need tar to unpack"

# --- detect target triple ------------------------------------------------------
os=$(uname -s)
arch=$(uname -m)
case "$os" in
	Darwin)
		case "$arch" in
			arm64 | aarch64) triple="aarch64-apple-darwin" ;;
			x86_64) triple="x86_64-apple-darwin" ;;
			*) err "unsupported macOS arch: $arch" ;;
		esac
		;;
	Linux)
		case "$arch" in
			x86_64 | amd64) triple="x86_64-unknown-linux-gnu" ;;
			*) err "unsupported Linux arch: $arch (only x86_64 for now)" ;;
		esac
		;;
	*)
		err "unsupported OS: $os — on Windows, download the .exe from the Releases page"
		;;
esac

url="https://github.com/$REPO/releases/latest/download/$BIN-$triple.tar.gz"

# --- download + unpack ---------------------------------------------------------
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
say "Downloading $BIN ($triple)…"
dl "$url" "$tmp/cli.tar.gz" || err "download failed: $url"

# --- verify integrity against the published SHA-256 ----------------------------
dl "$url.sha256" "$tmp/cli.tar.gz.sha256" 2>/dev/null ||
	err "could not fetch checksum ($url.sha256) — refusing to install unverified"
if command -v sha256sum >/dev/null 2>&1; then
	got=$(sha256sum "$tmp/cli.tar.gz" | awk '{print $1}')
else
	got=$(shasum -a 256 "$tmp/cli.tar.gz" | awk '{print $1}')
fi
want=$(awk '{print $1}' "$tmp/cli.tar.gz.sha256")
[ -n "$want" ] || err "checksum file was empty"
if [ "$got" != "$want" ]; then
	err "checksum mismatch — download may be corrupt or tampered
  expected $want
  got      $got"
fi
say "  checksum OK"

tar -xzf "$tmp/cli.tar.gz" -C "$tmp" || err "unpack failed"
[ -f "$tmp/$BIN" ] || err "archive did not contain $BIN"

# macOS: a freshly downloaded, unsigned binary is quarantined and won't run.
if [ "$os" = "Darwin" ]; then
	xattr -d com.apple.quarantine "$tmp/$BIN" 2>/dev/null || true
fi
chmod +x "$tmp/$BIN"

# --- install to PATH -----------------------------------------------------------
mkdir -p "$DEST" || err "cannot create $DEST"
mv "$tmp/$BIN" "$DEST/$BIN" || err "cannot install to $DEST (set XENOLITH_BIN to a writable dir)"

say ""
say "Installed $BIN to $DEST/$BIN"

case ":$PATH:" in
	*":$DEST:"*) on_path=1 ;;
	*) on_path=0 ;;
esac

if [ "$on_path" -eq 0 ]; then
	say ""
	say "$DEST is not on your PATH. Add it (then restart your shell):"
	say "  echo 'export PATH=\"$DEST:\$PATH\"' >> ~/.zshrc"
fi

say ""
say "Next steps:"
if [ "$on_path" -eq 1 ]; then
	say "  xenolith-installer-cli install            # download the SDK for this machine"
	say "  xenolith-installer-cli new myapp          # scaffold ./myapp"
	say "  xenolith-installer-cli build myapp --run  # build and launch it"
else
	say "  $DEST/xenolith-installer-cli install"
	say "  $DEST/xenolith-installer-cli new myapp"
	say "  $DEST/xenolith-installer-cli build myapp --run"
fi
