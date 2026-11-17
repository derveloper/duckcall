#!/bin/sh
# Rebuilds duckcall.icns from logo.svg. The icon is compiled into the binary and ends up in the
# app bundle that `duckcall install` creates, so run this and rebuild after changing the logo.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# sips rasterises an SVG at its own size, so give it the largest size needed.
sed 's/<svg /<svg width="512" height="512" /' logo.svg >"$work/logo.svg"
sips -s format png "$work/logo.svg" --out "$work/512.png" >/dev/null

mkdir "$work/duckcall.iconset"
for size in 16 32 128 256; do
	sips -z "$size" "$size" "$work/512.png" --out "$work/duckcall.iconset/icon_${size}x${size}.png" >/dev/null
	sips -z "$((size * 2))" "$((size * 2))" "$work/512.png" --out "$work/duckcall.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$work/duckcall.iconset" -o duckcall.icns
