#!/bin/sh
# 使用上游 Makefile 构建，仅在发行层改包名，降低后续同步成本。
set -eu
target=${1:?target required}
version=${2:?version required}
case "$version" in
  *[!0-9A-Za-z.+-]*|'') echo "invalid version" >&2; exit 1 ;;
esac
mkdir -p dist
case "$target" in
  darwin-arm64|darwin-amd64)
    make app VERSION="$version"
    # 不修改内部 executable，保留上游 CLI、自启动与 helper 的约定。
    mv magpie.app 'Token Proxy.app'
    codesign --force --deep --sign - 'Token Proxy.app'
    ditto -c -k --sequesterRsrc --keepParent 'Token Proxy.app' "dist/token-proxy-$target.zip"
    staging=$(mktemp -d)
    trap 'rm -rf "$staging"' EXIT
    ditto 'Token Proxy.app' "$staging/Token Proxy.app"
    ln -s /Applications "$staging/Applications"
    hdiutil create -volname 'Token Proxy' -srcfolder "$staging" -ov -format UDZO "dist/token-proxy-$target.dmg"
    ;;
  linux-amd64|linux-arm64)
    make release-linux VERSION="$version"
    mkdir -p dist/package
    cp "dist/magpie-$target" dist/package/token-proxy
    cp THIRD_PARTY_NOTICES LICENSE dist/package/
    tar -C dist/package -czf "dist/token-proxy-$target.tar.gz" token-proxy THIRD_PARTY_NOTICES LICENSE
    ;;
  windows)
    make release-windows VERSION="$version"
    for arch in amd64 arm64; do
      mkdir -p "dist/package-$arch"
      cp "dist/magpie-windows-$arch.exe" "dist/package-$arch/magpie.exe"
      cp THIRD_PARTY_NOTICES LICENSE "dist/package-$arch/"
      (cd "dist/package-$arch" && zip -q "../token-proxy-windows-$arch.zip" magpie.exe THIRD_PARTY_NOTICES LICENSE)
    done
    ;;
  *) echo "unsupported target: $target" >&2; exit 1 ;;
esac
