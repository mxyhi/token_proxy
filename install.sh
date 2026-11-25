#!/bin/sh
# Token Proxy 预发布安装器；只消费自有仓库发布的完整安装包。
set -eu

repo=mxyhi/token_proxy
tag=${TOKEN_PROXY_TAG:-}
bin=${TOKEN_PROXY_BIN_DIR:-$HOME/.local/bin}
die() { printf 'Token Proxy: %s\n' "$*" >&2; exit 1; }
case "${1:-}" in
  -h|--help)
    printf '%s\n' '安装最新 Token Proxy 预发布（需要 curl、jq）。' \
      'TOKEN_PROXY_TAG 可指定 prelease 标签；TOKEN_PROXY_BIN_DIR、TOKEN_PROXY_APP_DIR 可指定安装目录。' \
      'Linux 需要 glibc、GTK 3 和 WebKitGTK 4.1；Windows 请手动下载。'
    exit 0 ;;
esac
[ "$#" -eq 0 ] || die '不支持该参数；请查看 --help'
for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || die "请先安装 $tool"
done
case "$(uname -s)" in
  Darwin) os=darwin; ext=zip ;;
  Linux)
    [ -z "${TERMUX_VERSION:-}" ] || die '当前未发布 Android/Termux 包'
    os=linux; ext=tar.gz ;;
  *) die '仅支持 macOS 和 Linux；Windows 请从 GitHub Releases 下载' ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=arm64 ;;
  x86_64|amd64) arch=amd64 ;;
  *) die "不支持的 CPU：$(uname -m)" ;;
esac
file="token-proxy-$os-$arch.$ext"
case "$bin" in /*) ;; *) die '安装目录必须为绝对路径' ;; esac

tmp=$(mktemp -d)
stage=
cleanup() {
  rm -rf "$tmp"
  [ -z "$stage" ] || rm -rf "$stage"
}
trap cleanup EXIT
get() { curl -fsSL --retry 3 --connect-timeout 20 --max-time 600 "$@"; }
api="https://api.github.com/repos/$repo/releases"
if [ -n "$tag" ]; then
  printf '%s\n' "$tag" | LC_ALL=C grep -Eq '^prelease-[0-9]+\.[0-9]+\.[0-9]+-[0-9]+$' || die '无效的预发布标签'
  get -o "$tmp/release.json" "$api/tags/$tag" || die '读取指定预发布失败'
else
  get -o "$tmp/releases.json" "$api?per_page=100" || die '读取预发布列表失败'
  # /latest 排除 prerelease；按发布时间选，避免稳定版或草稿混入。
  jq -e '[.[] | select(.draft == false and .prerelease == true)
    | select(.tag_name | test("^prelease-[0-9]+\\.[0-9]+\\.[0-9]+-[0-9]+$"))]
    | sort_by(.published_at) | last' "$tmp/releases.json" > "$tmp/release.json" || die '找不到预发布版本'
fi
jq -e '.draft == false and .prerelease == true' "$tmp/release.json" >/dev/null || die '目标不是已发布的预发布版本'
tag=$(jq -er '.tag_name' "$tmp/release.json")
base="https://github.com/$repo/releases/download/$tag"
for asset in "$file" SHA256SUMS; do
  jq -e --arg name "$asset" --arg url "$base/$asset" \
    'any(.assets[]; .name == $name and .state == "uploaded" and .size > 0 and .browser_download_url == $url)' \
    "$tmp/release.json" >/dev/null || die "发布资产缺失或尚未上传：$asset"
  get -o "$tmp/$asset" "$base/$asset" || die "下载 $asset 失败"
done
expected=$(awk -v file="$file" '$2 == file {print $1}' "$tmp/SHA256SUMS")
printf '%s\n' "$expected" | LC_ALL=C grep -Eq '^[0-9a-f]{64}$' || die "缺少或无效的 $file 校验和"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$file")
else
  actual=$(shasum -a 256 "$tmp/$file")
fi
[ "${actual%% *}" = "$expected" ] || die "$file 校验失败；保留原安装"
mkdir -p "$bin"
if [ "$os" = darwin ]; then
  apps=${TOKEN_PROXY_APP_DIR:-/Applications}
  if [ -z "${TOKEN_PROXY_APP_DIR:-}" ] && [ ! -w "$apps" ]; then apps=$HOME/Applications; fi
  case "$apps" in /*) ;; *) die '应用目录必须为绝对路径' ;; esac
  mkdir -p "$apps"
  app="$apps/Token Proxy.app"
  # 同名的旧 Tauri 应用不能当作此发行版直接替换。
  if [ -e "$app" ] || [ -L "$app" ]; then
    id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist") || die '无法识别已有应用'
    [ "$id" = com.mxyhi.token-proxy.magpie ] && [ ! -L "$app" ] || die '同名应用不是此预发布；请用 TOKEN_PROXY_APP_DIR 指定其他目录'
  fi
  if [ -e "$bin/token-proxy" ] || [ -L "$bin/token-proxy" ]; then
    [ -L "$bin/token-proxy" ] && [ "$(readlink "$bin/token-proxy")" = "$app/Contents/MacOS/magpie" ] || die 'token-proxy 命令已存在；请指定其他 TOKEN_PROXY_BIN_DIR'
  fi
  stage=$(mktemp -d "$apps/.token-proxy.XXXXXX")
  ditto -x -k "$tmp/$file" "$stage/new"
  next="$stage/new/Token Proxy.app"
  [ -x "$next/Contents/MacOS/magpie" ] || die '安装包中缺少可执行文件'
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$next/Contents/Info.plist")" = com.mxyhi.token-proxy.magpie ] || die '安装包应用标识不符'
  [ ! -e "$app" ] || mv "$app" "$stage/previous.app"
  if ! mv "$next" "$app"; then
    # 回滚失败时保留备份，不能让退出清理删除唯一可用副本。
    if [ -e "$stage/previous.app" ]; then
      if ! mv "$stage/previous.app" "$app"; then
        saved=$stage; stage=; die "恢复失败，原应用保留在 $saved/previous.app"
      fi
    fi
    die '替换应用失败，已恢复原安装'
  fi
  ln -sf "$app/Contents/MacOS/magpie" "$bin/token-proxy"
  printf '已安装 %s：%s\n' "$tag" "$app"
else
  stage=$(mktemp -d "$bin/.token-proxy.XXXXXX")
  # 只解包发行包中约定的文件，不接受额外目录项。
  tar -xzf "$tmp/$file" -C "$stage" token-proxy LICENSE THIRD_PARTY_NOTICES
  [ -f "$stage/token-proxy" ] && [ ! -L "$stage/token-proxy" ] || die '安装包中缺少 token-proxy'
  chmod 755 "$stage/token-proxy"
  [ ! -d "$bin/token-proxy" ] || die '目标是目录，拒绝覆盖'
  mv -f "$stage/token-proxy" "$bin/token-proxy"
  printf '已安装 %s：%s/token-proxy\n' "$tag" "$bin"
  printf '%s\n' '运行需要 glibc、GTK 3 和 WebKitGTK 4.1。打开桌面：token-proxy app'
fi
case ":$PATH:" in *":$bin:"*) ;; *) printf '请将 %s 加入 PATH。\n' "$bin" ;; esac
