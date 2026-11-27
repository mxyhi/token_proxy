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
    rm -rf 'Token Proxy.app'
    mv magpie.app 'Token Proxy.app'
    codesign --force --deep --sign - 'Token Proxy.app'
    ditto -c -k --sequesterRsrc --keepParent 'Token Proxy.app' "dist/token-proxy-$target.zip"
    staging=$(mktemp -d)
    trap 'rm -rf "$staging"' EXIT
    ditto 'Token Proxy.app' "$staging/Token Proxy.app"
    ln -s /Applications "$staging/Applications"
    # hdiutil 按 -srcfolder 自估卷大小，在 macos-26-arm64 runner 上同一提交
    # 时而不够（No space left on device，写 MacOS/magpie 时），所以显式给
    # 源大小 1.5 倍加 32MB；UDZO 压缩后 dmg 体积不变。Resource busy 是 runner
    # 上 DiskArbitration 的瞬时错误：只重试这一步，不重编译，最多三次。
    dmg_mb=$(du -sm "$staging" | cut -f1)
    dmg_size=$((dmg_mb + dmg_mb / 2 + 32))m
    try=1
    until hdiutil create -volname 'Token Proxy' -srcfolder "$staging" -size "$dmg_size" -ov -format UDZO "dist/token-proxy-$target.dmg"; do
      echo "hdiutil create failed (try $try/3, size $dmg_size)" >&2
      df -h . "$staging" >&2 || true
      [ "$try" -lt 3 ] || exit 1
      try=$((try + 1))
      sleep 5
    done
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
