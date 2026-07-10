#!/usr/bin/env bash
# macOS 用: assets/icon.png から .icns を生成し、Dock アイコン付きの moterm.app を作る。
# 素のバイナリでは Dock に独自アイコンが出ないため、.app バンドルにまとめる。
# 前提: macOS の sips / iconutil（OS 同梱）と Rust。
#
# 使い方:
#   scripts/macos-bundle.sh                 リリースビルド → dist/moterm.app
#   scripts/macos-bundle.sh /Applications   ビルド → 指定先へ moterm.app を配置
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "このスクリプトは macOS 専用です（sips/iconutil を使います）。" >&2
  exit 1
fi

ICON_PNG="assets/icon.png"
[[ -f "$ICON_PNG" ]] || { echo "$ICON_PNG がありません。" >&2; exit 1; }

echo "== moterm.app バンドル生成 =="

# 1) ビルド
echo "cargo build -p mot-gui --release"
cargo build -p mot-gui --release
BIN="target/release/moterm"
[[ -f "$BIN" ]] || { echo "ビルド成果物が見つかりません: $BIN" >&2; exit 1; }

# 2) icon.png → icon.icns（各解像度を iconset にして iconutil で束ねる）
WORK="$(mktemp -d)"
ICONSET="$WORK/icon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 64 128 256 512; do
  sips -z "$size" "$size"        "$ICON_PNG" --out "$ICONSET/icon_${size}x${size}.png"   >/dev/null
  dbl=$((size * 2))
  sips -z "$dbl" "$dbl"          "$ICON_PNG" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$WORK/icon.icns"

# 3) .app 構造を組み立てる
APP="dist/moterm.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/moterm"
cp "$WORK/icon.icns" "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>moterm</string>
  <key>CFBundleDisplayName</key>     <string>moterm</string>
  <key>CFBundleIdentifier</key>      <string>com.motohidentaku.moterm</string>
  <key>CFBundleExecutable</key>      <string>moterm</string>
  <key>CFBundleIconFile</key>        <string>icon</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleShortVersionString</key> <string>0.1.0</string>
  <key>NSHighResolutionCapable</key> <true/>
</dict>
</plist>
PLIST

rm -rf "$WORK"

# 4) ad-hoc 署名
# 未署名の .app を quarantine 付き（ブラウザ DL）で開くと、Apple Silicon では
# 「壊れているため開けません」と表示される。ad-hoc 署名（-s -）を付けると
# この表示は解消する。ただし正式な公証ではないため、初回は右クリック→開く、
# もしくは `xattr -dr com.apple.quarantine moterm.app` が必要（README 参照）。
echo "codesign --force --deep --sign - $APP"
codesign --force --deep --options runtime --sign - "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

echo "生成: $APP （Dock アイコンは assets/icon.png 由来 / ad-hoc 署名済み）"

# 5) 任意の配置先
if [[ $# -ge 1 ]]; then
  DEST="$1"
  mkdir -p "$DEST"
  rm -rf "$DEST/moterm.app"
  cp -R "$APP" "$DEST/"
  echo "配置: $DEST/moterm.app"
fi
