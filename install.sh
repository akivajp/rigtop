#!/bin/sh
# Installs the rigtop release binaries for this machine:
#
#   curl -fsSL https://github.com/akivajp/rigtop/releases/latest/download/install.sh | sh
#
# Options after `sh -s --`, e.g. `... | sh -s -- --version v0.10.0`.
# Run `sh install.sh --help` for the list.
#
# Everything is inside main(), so a download cut short runs nothing.

set -eu

REPO="akivajp/rigtop"

usage() {
    cat <<'EOF'
Install rigtop (and rigtop-gui where the release has it) from GitHub Releases.

Usage: install.sh [options]

Options:
  --version <vX.Y.Z>  Release to install (default: the latest)
  --bin-dir <dir>     Where to put the binaries (default: ~/.local/bin)
  --tui-only          Do not install rigtop-gui
  -h, --help          Show this help

Supported: Linux x86_64 and aarch64 (glibc 2.35 or newer; aarch64 has the
TUI only) and macOS on Apple Silicon. Elsewhere, use `cargo install rigtop`.
EOF
}

say() {
    printf 'rigtop install: %s\n' "$1"
}

fail() {
    printf 'rigtop install: error: %s\n' "$1" >&2
    exit 1
}

# Prints the body of a URL to stdout.
fetch() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL --retry 3 "$1"
    elif command -v wget > /dev/null 2>&1; then
        wget -q -O - "$1"
    else
        fail "needs curl or wget"
    fi
}

# The tag of the latest release, from where /releases/latest redirects to
# (no API call, so no rate limit).
latest_tag() {
    url="https://github.com/$REPO/releases/latest"
    if command -v curl > /dev/null 2>&1; then
        target=$(curl -fsSL --retry 3 -o /dev/null -w '%{url_effective}' "$url")
    else
        target=$(wget -S --spider "$url" 2>&1 | sed -n 's/^ *[Ll]ocation: *\([^ ]*\).*/\1/p' | tail -n 1)
    fi
    tag=${target##*/tag/}
    tag=$(printf '%s' "$tag" | tr -d '\r')
    case "$tag" in
        v[0-9]*) printf '%s\n' "$tag" ;;
        *) fail "could not find the latest release (got '$target')" ;;
    esac
}

# The Rust target triple of the release archive for this machine.
detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            # The release binaries link against glibc.
            if ldd --version 2>&1 | grep -qi musl; then
                fail "musl-based systems (e.g. Alpine) are not supported; use 'cargo install rigtop'"
            fi
            case "$arch" in
                x86_64 | amd64) echo x86_64-unknown-linux-gnu ;;
                aarch64 | arm64) echo aarch64-unknown-linux-gnu ;;
                *) fail "no Linux build for $arch; use 'cargo install rigtop'" ;;
            esac
            ;;
        Darwin)
            # A shell running under Rosetta reports x86_64 on Apple Silicon.
            if [ "$arch" = arm64 ] || [ "$(sysctl -n hw.optional.arm64 2> /dev/null || true)" = 1 ]; then
                echo aarch64-apple-darwin
            else
                fail "no build for Intel Macs; use 'cargo install rigtop'"
            fi
            ;;
        MINGW* | MSYS* | CYGWIN*)
            fail "on Windows, use Scoop or the zip from the releases page (see the README)"
            ;;
        *) fail "no build for $os; use 'cargo install rigtop'" ;;
    esac
}

sha256_of() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum > /dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "needs sha256sum or shasum to check the download"
    fi
}

main() {
    version=""
    bin_dir="${HOME:?HOME is not set}/.local/bin"
    tui_only=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || fail "--version needs a value"
                version=$2
                shift 2
                ;;
            --bin-dir)
                [ $# -ge 2 ] || fail "--bin-dir needs a value"
                bin_dir=$2
                shift 2
                ;;
            --tui-only)
                tui_only=1
                shift
                ;;
            -h | --help)
                usage
                return 0
                ;;
            *) fail "unknown option: $1 (see --help)" ;;
        esac
    done

    target=$(detect_target)
    if [ -z "$version" ]; then
        version=$(latest_tag)
    fi
    case "$version" in
        v*) ;;
        *) version="v$version" ;;
    esac

    name="rigtop-$version-$target"
    base="https://github.com/$REPO/releases/download/$version"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM

    say "downloading $name.tar.gz"
    fetch "$base/$name.tar.gz" > "$tmp/$name.tar.gz" || fail "could not download $base/$name.tar.gz"
    fetch "$base/SHA256SUMS" > "$tmp/SHA256SUMS" || fail "could not download $base/SHA256SUMS"

    expected=$(awk -v f="$name.tar.gz" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$expected" ] || fail "SHA256SUMS has no entry for $name.tar.gz"
    actual=$(sha256_of "$tmp/$name.tar.gz")
    [ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz"

    tar xzf "$tmp/$name.tar.gz" -C "$tmp"
    mkdir -p "$bin_dir"
    for bin in rigtop rigtop-gui; do
        [ -f "$tmp/$name/$bin" ] || continue
        if [ "$bin" = rigtop-gui ] && [ "$tui_only" = 1 ]; then
            continue
        fi
        # Replace rather than overwrite, so a running copy keeps working.
        cp "$tmp/$name/$bin" "$bin_dir/.$bin.new"
        chmod 755 "$bin_dir/.$bin.new"
        mv -f "$bin_dir/.$bin.new" "$bin_dir/$bin"
        say "installed $bin_dir/$bin"
    done

    "$bin_dir/rigtop" --version > /dev/null 2>&1 \
        || fail "$bin_dir/rigtop does not run on this system (glibc 2.35 or newer is needed on Linux)"

    case ":$PATH:" in
        *":$bin_dir:"*)
            found=$(command -v rigtop || true)
            if [ "$found" != "$bin_dir/rigtop" ]; then
                say "note: '$found' comes first in PATH; remove it or put $bin_dir before it"
            fi
            ;;
        *) say "note: $bin_dir is not in PATH; add it, e.g. export PATH=\"$bin_dir:\$PATH\"" ;;
    esac
    say "done: $("$bin_dir/rigtop" --version)"
}

main "$@"
