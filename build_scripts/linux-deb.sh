#!/bin/sh
# Packs a Linux binary into a Debian/Ubuntu package.
# usage: build_scripts/linux-deb.sh [binary] [output-dir]
#   defaults: target/release/disk-usage and dist/
# Needs dpkg-deb and objdump (Debian/Ubuntu: dpkg, binutils).
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-$root/target/release/disk-usage}"
out="${2:-$root/dist}"
if [ ! -f "$bin" ]; then
    echo "error: binary not found: $bin" >&2
    echo "build it first (cargo build --release) or use build_scripts/build.sh" >&2
    exit 1
fi
mkdir -p "$out"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
arch="$(dpkg --print-architecture)"
# The newest glibc symbol the binary uses is the oldest libc6 it runs on.
glibc="$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"

pkg="$(mktemp -d)/disk-usage"
install -Dm755 "$bin" "$pkg/usr/bin/disk-usage"
install -Dm644 "$root/assets/disk-usage.desktop" "$pkg/usr/share/applications/disk-usage.desktop"
install -Dm644 "$root/assets/icon-256.png" "$pkg/usr/share/icons/hicolor/256x256/apps/disk-usage.png"
install -Dm644 "$root/LICENSE" "$pkg/usr/share/doc/disk-usage/copyright"

# X11, Wayland and OpenGL are loaded at run time (dlopen), so they are not in the binary's
# NEEDED list; any desktop already has them, the dependencies cover minimal installs.
mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: disk-usage
Version: $version
Architecture: $arch
Maintainer: MarketKernel <https://github.com/MarketKernel/disk-usage>
Installed-Size: $(du -sk "$pkg/usr" | cut -f1)
Depends: libc6 (>= $glibc), libgcc-s1 | libgcc1, libxkbcommon0, libgl1 | libegl1
Recommends: libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon-x11-0, libwayland-client0, xdg-desktop-portal
Section: utils
Homepage: https://github.com/MarketKernel/disk-usage
Priority: optional
Description: Lightweight disk usage visualizer with a sunburst chart
 Scans a folder or a whole disk and shows what takes up space as an
 interactive sunburst chart.
CONTROL

deb="$out/disk-usage_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$pkg" "$deb" >/dev/null
rm -rf "$(dirname "$pkg")"
echo "$deb"
