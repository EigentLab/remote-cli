#!/bin/sh
# Generates every icon from assets/logo/*.svg into assets/icons/, the Dashboard and the site. Run on a Mac (iconutil) with node.
# Usage: scripts/icons.sh   (from anywhere in the repo; commit the outputs)
set -eu
cd "$(git rev-parse --show-toplevel)"
logo=assets/logo
out=assets/icons
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out" dashboard/public

render() { npx -y @resvg/resvg-js-cli --no-system-font "$@"; } # render <svg|-> <png> [--fit-width N]
inner() { grep -v '<svg\|</svg>\|<title>' "$1"; }                # the drawing, without the <svg> wrapper
# The mark in the dark wordmark's colours (paper frame, ink screen): swap ink and paper.
inner_dark() { inner $logo/mark.svg | sed 's/#0A0D13/@/; s/#FBFEFF/#0A0D13/; s/@/#FBFEFF/'; }

# Square PNGs, the mark on a transparent ground. Multiples of 16 keep the mark's edges on whole pixels.
for s in 16 32 48 64 128 256 512 1024; do render $logo/mark.svg "$out/rcli-$s.png" --fit-width $s; done

# Windows .ico: PNG entries (Vista and later read them).
node -e '
const fs = require("fs"), [out, ...pngs] = process.argv.slice(1), data = pngs.map((p) => fs.readFileSync(p));
const head = Buffer.alloc(6 + 16 * data.length);
head.writeUInt16LE(1, 2); head.writeUInt16LE(data.length, 4);
let offset = head.length;
data.forEach((d, i) => {
  const e = 6 + 16 * i, size = d.readUInt32BE(16); // PNG width from the IHDR chunk
  head[e] = head[e + 1] = size >= 256 ? 0 : size;  // 0 means 256
  head.writeUInt16LE(1, e + 4); head.writeUInt16LE(32, e + 6);
  head.writeUInt32LE(d.length, e + 8); head.writeUInt32LE(offset, e + 12);
  offset += d.length;
});
fs.writeFileSync(out, Buffer.concat([head, ...data]));
' "$out/rcli.ico" "$out/rcli-16.png" "$out/rcli-32.png" "$out/rcli-48.png" "$out/rcli-256.png"

# macOS .icns: the mark on a rounded ink plate, on Apple's 1024 grid (824 plate, 100 margin).
cat > "$tmp/plate.svg" <<EOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <rect x="100" y="100" width="824" height="824" rx="185" fill="#0A0D13"/>
  <g transform="translate(256 256) scale(32)">
$(inner_dark)
  </g>
</svg>
EOF
iconset="$tmp/rcli.iconset"
mkdir "$iconset"
for s in 16 32 128 256 512; do
  render "$tmp/plate.svg" "$iconset/icon_${s}x${s}.png" --fit-width $s
  render "$tmp/plate.svg" "$iconset/icon_${s}x${s}@2x.png" --fit-width $((s * 2))
done
iconutil -c icns -o "$out/rcli.icns" "$iconset"

# README header: the wordmark for light and for dark GitHub themes (use both in a <picture>).
render $logo/wordmark.svg "$out/readme-header.png" --fit-height 128
render $logo/wordmark-dark.svg "$out/readme-header-dark.png" --fit-height 128

# Social preview: 1280x640, the dark wordmark centred on ink.
cat <<EOF | render - "$out/social-preview.png"
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 640" width="1280" height="640">
  <rect width="1280" height="640" fill="#0A0D13"/>
  <g transform="translate(298 224) scale(12)">
$(inner $logo/wordmark-dark.svg)
  </g>
</svg>
EOF

# Dashboard: favicons (Next serves app/icon.svg and app/favicon.ico) and the sidebar wordmark.
cp $logo/mark.svg dashboard/app/icon.svg
cp "$out/rcli.ico" dashboard/app/favicon.ico
cp $logo/wordmark-dark.svg dashboard/public/wordmark-dark.svg

# Site: favicon and og:image (the header logo reads assets/logo/ directly).
cp $logo/mark.svg site/public/favicon.svg
cp "$out/social-preview.png" site/public/social-preview.png
