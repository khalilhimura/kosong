#!/bin/sh
# kosong installer.
#
# Served from https://get.kosong.dev so that this works:
#
#   curl -fsSL https://get.kosong.dev | sh
#
# §16 permits `curl | sh` only because this script downloads a **fixed,
# checksum-verified release** rather than executing whatever is newest. If the
# checksum does not match, nothing is installed.
#
# You are reading this because you were sensible enough to look before piping a
# script into your shell. To install manually instead, see:
#   https://github.com/khalilhimura/kosong/releases

set -eu

REPO="khalilhimura/kosong"
# Pinned by the release workflow. A moving "latest" would mean two people
# running the same command on the same day could get different binaries.
VERSION="${KOSONG_VERSION:-latest}"
INSTALL_DIR="${KOSONG_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf '\nCould not install kosong: %s\n' "$*" >&2; exit 1; }

# --- What are we installing onto? -------------------------------------------

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) os_part="apple-darwin" ;;
        Linux)  os_part="unknown-linux-gnu" ;;
        *) fail "kosong supports macOS and Linux. This machine reports '$os'." ;;
    esac

    case "$arch" in
        arm64|aarch64) arch_part="aarch64" ;;
        x86_64|amd64)  arch_part="x86_64" ;;
        *) fail "kosong supports arm64 and x86_64. This machine reports '$arch'." ;;
    esac

    printf '%s-%s' "$arch_part" "$os_part"
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "this installer needs '$1', which is not installed."
}

# --- Verify before extracting -----------------------------------------------

# Prints the SHA-256 of a file, using whichever tool this machine has.
checksum_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        fail "no way to check the download is genuine (need shasum or sha256sum)."
    fi
}

main() {
    need curl
    need tar

    target="$(detect_target)"

    if [ "$VERSION" = "latest" ]; then
        base="https://github.com/$REPO/releases/latest/download"
    else
        base="https://github.com/$REPO/releases/download/v$VERSION"
    fi

    say "Installing kosong for $target."

    work="$(mktemp -d)"
    # Leave nothing behind, including on failure.
    trap 'rm -rf "$work"' EXIT INT TERM

    # The archive name embeds the version, which is unknown for "latest", so
    # the manifest is fetched first and the name read from it.
    say "  fetching checksums…"
    curl -fsSL "$base/SHA256SUMS" -o "$work/SHA256SUMS" \
        || fail "could not reach the release. Check your connection, or install manually from https://github.com/$REPO/releases"

    archive="$(grep -o "kosong-[^ ]*-${target}\.tar\.gz" "$work/SHA256SUMS" | head -1)"
    [ -n "$archive" ] || fail "this release has no build for $target."

    expected="$(grep " [ *]\{0,1\}${archive}\$" "$work/SHA256SUMS" | cut -d' ' -f1 | head -1)"
    [ -n "$expected" ] || fail "the checksum manifest has no entry for $archive."

    say "  downloading $archive…"
    curl -fsSL "$base/$archive" -o "$work/$archive" || fail "the download failed."

    say "  checking it is genuine…"
    actual="$(checksum_of "$work/$archive")"
    if [ "$actual" != "$expected" ]; then
        fail "the download does not match its checksum, so it was NOT installed.

  expected  $expected
  got       $actual

This can mean a corrupted download, or that something interfered with it.
Try again. If it happens twice, please report it."
    fi

    say "  unpacking…"
    tar -xzf "$work/$archive" -C "$work"

    binary="$(find "$work" -type f -name kosong -perm -u+x | head -1)"
    [ -n "$binary" ] || fail "the archive did not contain a kosong binary."

    mkdir -p "$INSTALL_DIR"
    install -m 755 "$binary" "$INSTALL_DIR/kosong" 2>/dev/null \
        || { cp "$binary" "$INSTALL_DIR/kosong" && chmod 755 "$INSTALL_DIR/kosong"; }

    # --- Does it work, and can they run it? ---------------------------------

    installed_version="$("$INSTALL_DIR/kosong" --version 2>/dev/null || true)"
    [ -n "$installed_version" ] || fail "kosong was copied to $INSTALL_DIR but will not run."

    say ""
    say "Installed $installed_version to $INSTALL_DIR/kosong"

    if command -v kosong >/dev/null 2>&1 && [ "$(command -v kosong)" = "$INSTALL_DIR/kosong" ]; then
        say ""
        say "Next: kosong start"
    else
        # Saying "add it to your PATH" to a beginner is not an instruction.
        # This says exactly which line to add and where.
        shell_name="$(basename "${SHELL:-sh}")"
        case "$shell_name" in
            zsh)  profile="$HOME/.zshrc" ;;
            bash) profile="$HOME/.bashrc" ;;
            fish) profile="$HOME/.config/fish/config.fish" ;;
            *)    profile="your shell's startup file" ;;
        esac

        say ""
        say "One more step: your shell cannot find kosong yet."
        say ""
        if [ "$shell_name" = "fish" ]; then
            say "  echo 'fish_add_path $INSTALL_DIR' >> $profile"
        else
            say "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> $profile"
        fi
        say ""
        say "Then open a new terminal and run: kosong start"
    fi
}

main "$@"
