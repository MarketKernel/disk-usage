#!/bin/sh
# Local release build into dist/.
#
#   build_scripts/build.sh               build for this machine
#   build_scripts/build.sh --universal   macOS: Apple Silicon + Intel in one binary
#   build_scripts/build.sh --run         build, then launch
#
# macOS gets "dist/Disk Usage.app", Linux and Windows (Git Bash) a plain binary.
set -eu

cd "$(dirname "$0")/.."

universal=0
run=0
for arg in "$@"; do
    case "$arg" in
        --universal) universal=1 ;;
        --run) run=1 ;;
        -h | --help)
            sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (see build_scripts/build.sh --help)" >&2
            exit 1
            ;;
    esac
done

case "$(uname -s)" in
    Darwin) os=macos ;;
    Linux) os=linux ;;
    MINGW* | MSYS* | CYGWIN*) os=windows ;;
    *) os=other ;;
esac

if [ "$universal" = 1 ] && [ "$os" != macos ]; then
    echo "--universal only applies to macOS" >&2
    exit 1
fi

mkdir -p dist

if [ "$universal" = 1 ]; then
    targets="aarch64-apple-darwin x86_64-apple-darwin"
    installed="$(rustup target list --installed)"
    for t in $targets; do
        if ! echo "$installed" | grep -qx "$t"; then
            echo "installing Rust target $t"
            rustup target add "$t"
        fi
        cargo build --release --target "$t"
    done
    bin=dist/disk-usage
    lipo -create -output "$bin" \
        target/aarch64-apple-darwin/release/disk-usage \
        target/x86_64-apple-darwin/release/disk-usage
else
    cargo build --release
    if [ "$os" = windows ]; then
        bin=dist/disk-usage.exe
        cp target/release/disk-usage.exe "$bin"
    else
        bin=dist/disk-usage
        cp target/release/disk-usage "$bin"
    fi
fi

size="$(wc -c < "$bin" | tr -d ' ')"
echo "binary: $bin ($(awk "BEGIN { printf \"%.2f\", $size / 1048576 }") MiB)"

if [ "$os" = macos ]; then
    app="$(build_scripts/macos-bundle.sh "$bin" dist)"
    rm "$bin"
    echo "app:    $app"
    if [ "$run" = 1 ]; then
        open "$app"
    fi
elif [ "$run" = 1 ]; then
    "./$bin"
fi
