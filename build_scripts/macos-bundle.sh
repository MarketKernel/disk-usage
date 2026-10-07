#!/bin/sh
# Wraps a macOS binary into "Disk Usage.app".
# usage: build_scripts/macos-bundle.sh [binary] [output-dir]
#   defaults: target/release/disk-usage and dist/
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
app="$out/Disk Usage.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/disk-usage"

# Icon set from the 1024px master.
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double "$root/assets/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Disk Usage</string>
    <key>CFBundleDisplayName</key><string>Disk Usage</string>
    <key>CFBundleIdentifier</key><string>io.github.disk-usage</string>
    <key>CFBundleExecutable</key><string>disk-usage</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature so Apple Silicon runs it after unzipping (still needs right-click → Open
# the first time, since it is not notarized).
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "$app"
