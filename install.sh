#!/bin/sh
# Install, update or remove the prebuilt rune-tui on Linux or macOS.
#
#   ./install.sh                         # newest release into ~/.local (root: /usr/local)
#   ./install.sh --version v0.2.0        # a given release
#   ./install.sh --prefix /opt/rune      # somewhere else
#   ./install.sh --archive rune-tui-x86_64-unknown-linux-gnu.tar.gz   # no download
#   ./install.sh --uninstall             # with the same --prefix as the install
#
# Every archive is checked against the release's SHA256SUMS and the program is
# run once before anything is put in place; nothing is left behind in a
# temporary directory, whatever happens. Your session, messages, settings and
# plugins (~/.local/share/rune, ~/.config/rune) are never touched.
#
# The files installed are the ones `make install` installs, so either method
# can update or remove what the other put there.
set -eu

REPO=${RUNE_REPO:-ThanapatFungpharit/Rune}
VERSION=latest
PREFIX=
ARCHIVE=
UNINSTALL=0
VERIFY=1

usage() {
    cat <<'EOF'
Usage: install.sh [OPTIONS]

Install the prebuilt rune-tui for this machine (Linux or macOS; x86_64 or
arm64) from a GitHub release.

Options:
  --version TAG     release to install, e.g. v0.2.0 (default: latest)
  --prefix DIR      install under DIR/bin, DIR/share (default: ~/.local,
                    /usr/local when run as root)
  --archive FILE    install this rune-tui-<target>.tar.gz instead of
                    downloading; checked against SHA256SUMS or FILE.sha256
                    beside it
  --no-verify       with --archive: allow one that has no checksum file
  --repo OWNER/NAME GitHub repository to download from (default:
                    ThanapatFungpharit/Rune; or $RUNE_REPO)
  --uninstall       remove rune-tui from the prefix (your data stays)
  -h, --help        show this help

A private repository needs the GitHub CLI, signed in (`gh auth login`); it is
used automatically when it is.
EOF
}

say() { printf '%s\n' "$*"; }
warn() { printf 'install.sh: %s\n' "$*" >&2; }
die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
    case $1 in
    --version | --prefix | --archive | --repo)
        [ $# -ge 2 ] && [ -n "$2" ] || die "$1 needs a value (see --help)"
        case $1 in
        --version) VERSION=$2 ;;
        --prefix) PREFIX=$2 ;;
        --archive) ARCHIVE=$2 ;;
        --repo) REPO=$2 ;;
        esac
        shift 2
        ;;
    --version=* | --prefix=* | --archive=* | --repo=*)
        val=${1#*=}
        [ -n "$val" ] || die "${1%%=*} needs a value (see --help)"
        case $1 in
        --version=*) VERSION=$val ;;
        --prefix=*) PREFIX=$val ;;
        --archive=*) ARCHIVE=$val ;;
        --repo=*) REPO=$val ;;
        esac
        shift
        ;;
    --uninstall) UNINSTALL=1 && shift ;;
    --no-verify) VERIFY=0 && shift ;;
    -h | --help) usage && exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
    esac
done

case $VERSION in
latest | v[0-9]*) ;;
[0-9]*) VERSION=v$VERSION ;;
*) die "--version: expected a tag like v0.2.0 or latest, got '$VERSION'" ;;
esac
case $REPO in
*/*) ;;
*) die "--repo: expected OWNER/NAME, got '$REPO'" ;;
esac
[ "$VERIFY" = 1 ] || [ -n "$ARCHIVE" ] || die "--no-verify only applies to --archive: downloads are always checked"

# ---------------------------------------------------------------- platform

os=$(uname -s)
arch=$(uname -m)
case $os in
Linux) os=linux ;;
Darwin) os=macos ;;
*) die "no prebuilt rune-tui for $os (Linux and macOS are supported); see README.md to build it" ;;
esac
case $arch in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) die "no prebuilt rune-tui for $arch on $os (x86_64 and arm64 are supported); see README.md to build it" ;;
esac
# A shell running under Rosetta reports x86_64 on Apple silicon: install the native build.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
    arch=aarch64
fi
case $os in
linux) target=$arch-unknown-linux-gnu ;;
macos) target=$arch-apple-darwin ;;
esac
name=rune-tui-$target

if [ -z "$PREFIX" ]; then
    if [ "$(id -u)" = 0 ]; then
        PREFIX=/usr/local
    else
        [ -n "${HOME:-}" ] || die "HOME is not set; give --prefix"
        PREFIX=$HOME/.local
    fi
fi
case $PREFIX in
/*) ;;
*) PREFIX=$(pwd)/$PREFIX ;;
esac
PREFIX=${PREFIX%/}
[ -n "$PREFIX" ] || die "--prefix / is not allowed"

# What gets installed, relative to the prefix (the same files as `make install`).
# The desktop entry and icon only mean something on a freedesktop desktop.
FILES="bin/rune-tui share/man/man1/rune-tui.1 share/licenses/rune/LICENSE"
DESKTOP_FILES="share/applications/rune-tui.desktop share/icons/hicolor/scalable/apps/rune-tui.svg"
[ "$os" = linux ] && FILES="$FILES $DESKTOP_FILES"

refresh_desktop() {
    [ "$os" = linux ] || return 0
    if have update-desktop-database && [ -d "$PREFIX/share/applications" ]; then
        update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
    fi
    if have gtk-update-icon-cache && [ -d "$PREFIX/share/icons/hicolor" ]; then
        gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
    fi
}

data_note() {
    say "Your login, messages, settings and plugins were not touched:"
    say "  ${XDG_DATA_HOME:-~/.local/share}/rune, ${XDG_CONFIG_HOME:-~/.config}/rune, ${XDG_CACHE_HOME:-~/.cache}/rune"
}

# ---------------------------------------------------------------- uninstall

if [ "$UNINSTALL" = 1 ]; then
    removed=0
    # Desktop files too on macOS, in case `make install` put them there.
    for f in $FILES $DESKTOP_FILES; do
        if [ -e "$PREFIX/$f" ] || [ -L "$PREFIX/$f" ]; then
            rm -f "$PREFIX/$f" || die "cannot remove $PREFIX/$f (run as its owner, e.g. with sudo)"
            say "removed $PREFIX/$f"
            removed=1
        fi
    done
    rmdir "$PREFIX/share/licenses/rune" 2>/dev/null || true
    [ "$removed" = 1 ] || say "rune-tui is not installed under $PREFIX (give the --prefix it was installed with)"
    refresh_desktop
    if [ "$removed" = 1 ] && have rune-tui; then
        warn "another rune-tui is still on your PATH: $(command -v rune-tui)"
    fi
    data_note
    exit 0
fi

# ---------------------------------------------------------------- tools

for tool in tar gzip uname mktemp; do
    have "$tool" || die "$tool is needed and not installed"
done
if have sha256sum; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif have shasum; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif have openssl; then
    sha256() { openssl dgst -sha256 -r "$1" | cut -d' ' -f1; }
else
    die "sha256sum, shasum or openssl is needed to check the download"
fi

downloader=
if [ -z "$ARCHIVE" ]; then
    # gh first: it is the only way into a private repository, and it is signed in when present.
    if have gh && gh auth status --hostname github.com >/dev/null 2>&1; then
        downloader=gh
    elif have curl; then
        downloader=curl
    elif have wget; then
        downloader=wget
    else
        die "curl or wget is needed to download rune-tui (or give --archive)"
    fi
fi

# ---------------------------------------------------------------- workspace

tmp=
cleanup() {
    [ -n "$tmp" ] && rm -rf "$tmp"
    # Half-copied files from an interrupted install (see put()).
    for f in $FILES; do
        rm -f "$PREFIX/$f.rune-new.$$" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t rune-install) || die "cannot create a temporary directory"

# ---------------------------------------------------------------- fetch

fetch() { # fetch FILE: download one release asset into $tmp
    if [ "$VERSION" = latest ]; then
        url=https://github.com/$REPO/releases/latest/download/$1
    else
        url=https://github.com/$REPO/releases/download/$VERSION/$1
    fi
    case $downloader in
    gh)
        if [ "$VERSION" = latest ]; then
            gh release download --repo "$REPO" --pattern "$1" --dir "$tmp" 2>"$tmp/err"
        else
            gh release download "$VERSION" --repo "$REPO" --pattern "$1" --dir "$tmp" 2>"$tmp/err"
        fi
        ;;
    curl) curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tmp/$1" "$url" 2>"$tmp/err" ;;
    wget) wget --https-only -q -O "$tmp/$1" "$url" 2>"$tmp/err" ;;
    esac || {
        sed 's/^/  /' "$tmp/err" >&2
        if [ "$downloader" = gh ]; then
            die "cannot download $1 from release $VERSION of $REPO"
        fi
        die "cannot download $url
  (no such release or file for $target, or the repository is private:
  install the GitHub CLI and run 'gh auth login', or download the archive
  yourself and pass --archive)"
    }
}

if [ -n "$ARCHIVE" ]; then
    [ -f "$ARCHIVE" ] || die "no such file: $ARCHIVE"
    archive_name=$(basename "$ARCHIVE")
    cp "$ARCHIVE" "$tmp/$archive_name"
    dir=$(dirname "$ARCHIVE")
    if [ -f "$dir/SHA256SUMS" ]; then
        cp "$dir/SHA256SUMS" "$tmp/sums"
    elif [ -f "$ARCHIVE.sha256" ]; then
        cp "$ARCHIVE.sha256" "$tmp/sums"
    elif [ "$VERIFY" = 1 ]; then
        die "no SHA256SUMS or $archive_name.sha256 beside $ARCHIVE to check it against (--no-verify skips the check)"
    fi
    say "Installing rune-tui from $ARCHIVE"
else
    archive_name=$name.tar.gz
    say "Downloading rune-tui ($VERSION) for $target"
    fetch "$archive_name"
    fetch SHA256SUMS
    mv "$tmp/SHA256SUMS" "$tmp/sums"
fi

# ---------------------------------------------------------------- verify

if [ -f "$tmp/sums" ]; then
    # Lines are "<hash>  <file>" (or "<hash> *<file>"): find ours by name.
    want=$(awk -v f="$archive_name" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; exit }' "$tmp/sums")
    [ -n "$want" ] || die "the checksum list has no entry for $archive_name"
    got=$(sha256 "$tmp/$archive_name")
    [ "$want" = "$got" ] || die "checksum mismatch for $archive_name: expected $want, got $got (corrupted or tampered download; nothing was installed)"
    say "Checksum OK"
else
    warn "not checking $archive_name: no checksum (--no-verify)"
fi

# Only rune-tui-<target>/… inside: no absolute paths, no "..", no other folder.
top=$(gzip -dc "$tmp/$archive_name" 2>/dev/null | tar -tf - 2>/dev/null | head -n 1 | cut -d/ -f1)
case $top in
rune-tui-*-*) ;;
*) die "$archive_name does not look like a rune-tui archive" ;;
esac
[ "$top" = "$name" ] || die "$archive_name is built for ${top#rune-tui-}, but this machine needs $target"
bad=$(gzip -dc "$tmp/$archive_name" | tar -tf - | awk -v t="$top" '
    { p = $0; sub(/\/$/, "", p) }
    p != t && (index(p, t "/") != 1 || p ~ /(^|\/)\.\.(\/|$)/) { print; exit }')
[ -z "$bad" ] || die "$archive_name holds an unexpected path ($bad); nothing was installed"

mkdir "$tmp/x"
gzip -dc "$tmp/$archive_name" | (cd "$tmp/x" && tar -xf -) ||
    die "cannot unpack $archive_name"
src=$tmp/x/$top
for f in $FILES; do
    [ -f "$src/$f" ] && [ ! -L "$src/$f" ] || die "$archive_name is missing $f"
done

# Run it once before installing: catches a wrong architecture or a C library
# too old (or musl) for the build, with the old install still in place.
chmod 755 "$src/bin/rune-tui"
if ! new_version=$("$src/bin/rune-tui" --version 2>"$tmp/err"); then
    sed 's/^/  /' "$tmp/err" >&2
    if [ "$os" = linux ]; then
        if ldd --version 2>&1 | grep -qi musl; then
            die "the prebuilt rune-tui needs glibc and this system uses musl; build it from source (README.md)"
        fi
        libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || echo "unknown glibc")
        die "the prebuilt rune-tui does not run here ($libc; it needs glibc 2.35 or newer); build it from source (README.md)"
    fi
    die "the prebuilt rune-tui does not run on this Mac (it needs macOS 11 or newer); build it from source (README.md)"
fi
case $new_version in
"rune-tui "[0-9]*) ;;
*) die "unexpected output from rune-tui --version: $new_version" ;;
esac
if [ "$VERSION" != latest ] && [ "rune-tui ${VERSION#v}" != "$new_version" ]; then
    die "release $VERSION holds $new_version; nothing was installed"
fi

# ---------------------------------------------------------------- install

old_version=
if [ -x "$PREFIX/bin/rune-tui" ]; then
    old_version=$("$PREFIX/bin/rune-tui" --version 2>/dev/null || true)
fi

mkdir -p "$PREFIX/bin" 2>/dev/null && [ -w "$PREFIX/bin" ] ||
    die "cannot write to $PREFIX/bin: run as its owner (e.g. sudo) or choose another --prefix"

umask 022
put() { # put REL MODE: copy beside the target, then rename over it
    dest=$PREFIX/$1
    mkdir -p "$(dirname "$dest")" || die "cannot create $(dirname "$dest")"
    if ! { cp "$src/$1" "$dest.rune-new.$$" && chmod "$2" "$dest.rune-new.$$" &&
        mv -f "$dest.rune-new.$$" "$dest"; }; then
        rm -f "$dest.rune-new.$$"
        die "cannot install $dest"
    fi
}
# The program last, so it never runs beside an older man page or license; the
# rename replaces it even while an older rune-tui is running.
for f in $FILES; do
    [ "$f" = bin/rune-tui ] || put "$f" 644
done
put bin/rune-tui 755
# macOS: a quarantine flag (if the archive came from a browser) would stop it opening.
if [ "$os" = macos ] && have xattr; then
    xattr -d com.apple.quarantine "$PREFIX/bin/rune-tui" 2>/dev/null || true
fi
refresh_desktop

# ---------------------------------------------------------------- check

installed=$("$PREFIX/bin/rune-tui" --version 2>/dev/null) || die "$PREFIX/bin/rune-tui does not run after installing"
[ "$installed" = "$new_version" ] || die "$PREFIX/bin/rune-tui reports '$installed', expected '$new_version'"

if [ -n "$old_version" ] && [ "$old_version" != "$new_version" ]; then
    say "Updated $old_version -> $new_version in $PREFIX"
else
    say "Installed $new_version in $PREFIX"
fi
for f in $FILES; do say "  $PREFIX/$f"; done

on_path=$(command -v rune-tui 2>/dev/null || true)
if [ -z "$on_path" ]; then
    say ""
    say "$PREFIX/bin is not on your PATH; add it to run rune-tui by name, e.g.:"
    say "  echo 'export PATH=\"$PREFIX/bin:\$PATH\"' >> ~/.profile"
elif [ "$on_path" != "$PREFIX/bin/rune-tui" ]; then
    say ""
    warn "'rune-tui' runs $on_path ($("$on_path" --version 2>/dev/null || echo 'unknown version')), which comes earlier on your PATH than $PREFIX/bin"
fi

# Optional programs rune-tui uses when they are there.
missing=
lack() { missing="$missing
  $1"; }
have mpv || lack "mpv (video and audio)"
have git || lack "git (plugins from Git)"
if [ "$os" = linux ]; then
    have xdg-open || lack "xdg-open (opening files and links)"
    have notify-send || lack "notify-send (notifications)"
    have wl-copy || have xclip || have xsel || lack "wl-copy, xclip or xsel (copying; else the terminal's OSC 52)"
fi
if [ -n "$missing" ]; then
    say ""
    say "Optional, not found:$missing"
fi

say ""
say "rune-tui also needs LINE's ltsm.wasm (not distributed with it): it is found in"
say "Chrome, Chromium, Brave, Edge or Vivaldi when the LINE extension is installed"
say "there; otherwise see 'Getting ltsm.wasm' in README.md. Run rune-tui to log in."
