#!/bin/sh
# Build rune-tui and pack it the way install.sh deploys it:
#
#   scripts/dist.sh [OUTDIR]           # this machine's target, into OUTDIR (default dist)
#   TARGET=x86_64-apple-darwin scripts/dist.sh
#
# Writes OUTDIR/rune-tui-<target>.tar.gz and OUTDIR/rune-tui-<target>.tar.gz.sha256.
# The archive holds one folder, rune-tui-<target>/, laid out like an install
# prefix (bin/, share/man/…, share/applications/…, share/icons/…,
# share/licenses/rune/) by `make install`, so both install methods put the
# same files in the same places. The version is not in the file name, so
# releases/latest/download/<name> always finds the newest release.
#
# For a Windows target (*-windows-msvc, run from Git Bash) it writes
# OUTDIR/rune-tui-<target>.zip (+ .zip.sha256) instead, for install.ps1:
# rune-tui-<target>/bin/rune-tui.exe, LICENSE.txt, README.md and
# README-windows.txt (packaging/). The C runtime is linked statically, so the
# program runs without the Visual C++ Redistributable.
set -eu

cd "$(dirname "$0")/.."
out=${1:-dist}
target=${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
[ -n "$target" ] || { echo "dist.sh: cannot tell the target (is rustc installed?)" >&2; exit 1; }
name=rune-tui-$target

# Without debug symbols, stripped by the linker so a macOS build keeps a valid
# ad-hoc signature. Exported: `make install` builds again and must not switch
# back to the unstripped binary cargo also keeps.
export CARGO_PROFILE_RELEASE_STRIP=symbols

# "<hash>  <file>" whichever tool there is (Git Bash's sha256sum would write "*<file>").
checksum() { # checksum DIR FILE
    if command -v sha256sum >/dev/null 2>&1; then
        hash=$(sha256sum <"$1/$2" | cut -d' ' -f1)
    else
        hash=$(shasum -a 256 <"$1/$2" | cut -d' ' -f1)
    fi
    printf '%s  %s\n' "$hash" "$2" >"$1/$2.sha256"
}

case $target in
*-windows-msvc)
    # No `make install` here (no install(1), no man page or desktop entry on Windows).
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
    cargo build --release --locked --features tui --bin rune-tui ${TARGET:+--target "$TARGET"}
    bin=target/${TARGET:+$TARGET/}release/rune-tui.exe
    stage=$(mktemp -d)
    trap 'rm -rf "$stage"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    mkdir -p "$stage/$name/bin"
    cp "$bin" "$stage/$name/bin/rune-tui.exe"
    cp LICENSE "$stage/$name/LICENSE.txt"
    cp README.md "$stage/$name/README.md"
    cp packaging/README-windows.txt "$stage/$name/README-windows.txt"
    mkdir -p "$out"
    out_abs=$(cd "$out" && pwd)
    rm -f "$out_abs/$name.zip.part"
    if command -v 7z >/dev/null 2>&1; then
        (cd "$stage" && 7z a -tzip -mx=9 -bd -bso0 "$out_abs/$name.zip.part" "$name") >/dev/null
    else
        (cd "$stage" && zip -qr9X "$out_abs/$name.zip.part" "$name")
    fi
    mv -f "$out_abs/$name.zip.part" "$out_abs/$name.zip"
    checksum "$out" "$name.zip"
    echo "$out/$name.zip"
    exit 0
    ;;
*-apple-darwin)
    # Apple's linker ad-hoc signs only arm64 binaries by default; ask for it on
    # x86_64 too, so both releases carry a valid signature (the workflow checks).
    export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-adhoc_codesign"
    ;;
esac

make --no-print-directory build TARGET="${TARGET:-}"

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# PREFIX= puts bin/ and share/ straight under the folder; DESTDIR skips the
# desktop-cache refresh. Its PATH note does not apply here.
make --no-print-directory install TARGET="${TARGET:-}" PREFIX= DESTDIR="$stage/$name" >/dev/null
cp README.md "$stage/$name/README.md"
chmod -R u=rwX,go=rX "$stage/$name"

mkdir -p "$out"
# Owner and group are meaningless on the installing machine.
tar -C "$stage" -cf - "$name" | gzip -9n >"$out/$name.tar.gz.part"
mv -f "$out/$name.tar.gz.part" "$out/$name.tar.gz"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$out" && sha256sum "$name.tar.gz") >"$out/$name.tar.gz.sha256"
else
    (cd "$out" && shasum -a 256 "$name.tar.gz") >"$out/$name.tar.gz.sha256"
fi
echo "$out/$name.tar.gz"
