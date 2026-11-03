#!/bin/sh
# Linux x64 bootstrap. This script contains the public key, never the signing seed.
# Usage: curl -fsSL https://raw.githubusercontent.com/qmtdlt/sshm-releases/main/install.sh | sh
set -eu

apply_code() {
    cat <<'APPLY'
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 022
source_file=$1
expected=$2
destination=$3
case "$expected" in *[!0-9a-f]*|'') exit 1;; esac
[ "${#expected}" = 64 ] || exit 1
if [ "$destination" = /usr/local/bin ]; then
    [ "$(id -u)" = 0 ] || exit 1
    for dir in /usr /usr/local /usr/local/bin; do
        [ ! -L "$dir" ] || { echo "拒绝符号链接安装目录：$dir" >&2; exit 1; }
        if [ -e "$dir" ]; then
            [ -d "$dir" ] && [ "$(stat -c %u "$dir")" = 0 ] || exit 1
            mode=$(stat -c %a "$dir")
            [ $((0$mode & 0022)) = 0 ] || { echo "安装目录可被普通用户写入：$dir" >&2; exit 1; }
        fi
    done
fi
mkdir -p "$destination"
[ ! -L "$destination" ] || exit 1
lock="$destination/.sshm-install.lock"
mkdir "$lock" || { echo '另一个安装正在进行；若曾被强制终止，请检查后清理 .sshm-install.lock。' >&2; exit 1; }
entry="$destination/sshm"
receipt="$destination/.sshm-install.sha256"
changed=0
had_old=0
cleanup() {
    result=$?
    if [ "$result" != 0 ] && [ "$changed" = 1 ]; then
        if [ "$had_old" = 1 ]; then
            mv -f "$lock/old" "$entry"
            mv -f "$lock/old-receipt" "$receipt"
        else
            rm -f "$entry" "$receipt"
        fi
    fi
    rm -rf "$lock"
    exit "$result"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
if [ -e "$entry" ] || [ -L "$entry" ]; then
    [ -f "$entry" ] && [ ! -L "$entry" ] && [ -f "$receipt" ] && [ ! -L "$receipt" ] || { echo '已有其他方式安装的 sshm，拒绝覆盖；请先按原方式卸载。' >&2; exit 1; }
    old_hash=$(sha256sum "$entry" | awk '{print $1}')
    [ "$old_hash" = "$(cat "$receipt")" ] || { echo '现有程序已被修改，拒绝覆盖。' >&2; exit 1; }
    if [ "$old_hash" = "$expected" ]; then echo '相同版本内容已安装。'; exit 0; fi
    cp -p "$entry" "$lock/old"
    cp -p "$receipt" "$lock/old-receipt"
    had_old=1
elif [ -e "$receipt" ] || [ -L "$receipt" ]; then
    echo '安装收据与程序不一致，请检查后重试。' >&2
    exit 1
fi
cp "$source_file" "$lock/new"
actual=$(sha256sum "$lock/new" | awk '{print $1}')
[ "$actual" = "$expected" ] || { echo '程序在授权期间被修改，已停止。' >&2; exit 1; }
chmod 755 "$lock/new"
printf '%s\n' "$expected" > "$lock/new-receipt"
chmod 644 "$lock/new-receipt"
# Same filesystem rename: do not truncate a running executable.
changed=1
mv -f "$lock/new" "$entry"
mv -f "$lock/new-receipt" "$receipt"
APPLY
}

main() {
    fail() { printf 'sshm 安装失败：%s\n' "$*" >&2; exit 1; }
    [ "$(uname -s)" = Linux ] || fail '此脚本仅适用于 Linux。'
    case "$(uname -m)" in x86_64|amd64) ;; *) fail '首版仅支持 Linux x64；此架构尚未提供安装包。';; esac
    for tool in curl tar gzip openssl sha256sum mktemp awk wc chmod id ssh; do
        command -v "$tool" >/dev/null 2>&1 || fail "缺少 $tool；请先安装 curl、OpenSSL 3、tar 和 OpenSSH 客户端。"
    done
    case "$(openssl version)" in 'OpenSSL 3.'*|'OpenSSL '[4-9].*) ;; *) fail '签名验证需要 OpenSSL 3 或更新版本。';; esac
    install_dir=${SSHM_INSTALL_DIR:-/usr/local/bin}
    case "$install_dir" in /*) ;; *) fail 'SSHM_INSTALL_DIR 必须是绝对路径。';; esac
    if [ -n "${SSHM_VERSION:-}" ]; then
        printf '%s\n' "$SSHM_VERSION" | awk '/^[0-9]+[.][0-9]+[.][0-9]+$/ {ok=1} END {exit !ok}' || fail 'SSHM_VERSION 应为稳定版本号，例如 0.4.0。'
        base="https://github.com/qmtdlt/sshm-releases/releases/download/v$SSHM_VERSION"
    else
        base='https://github.com/qmtdlt/sshm-releases/releases/latest/download'
    fi
    umask 077
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    trap 'exit 1' HUP INT TERM
    download() {
        curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
            --connect-timeout 10 --max-time 180 --max-filesize "$3" \
            --output "$work/$2" "$base/$1" || fail '下载失败；请检查网络、版本是否已发布以及 GitHub 是否可访问。'
    }
    cat > "$work/key.pem" <<'PUBLIC_KEY'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAAGEFko1x5cvfXNVDsr3R7WAjpIIlv7vnqeMlbQN4rA4=
-----END PUBLIC KEY-----
PUBLIC_KEY
    download linux-SHA256SUMS checksums 4096
    download linux-SHA256SUMS.sig signature 64
    [ "$(wc -c < "$work/signature" | tr -d ' ')" = 64 ] || fail '签名长度无效。'
    openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin \
        -in "$work/checksums" -sigfile "$work/signature" >/dev/null 2>&1 || fail '发布清单签名不正确，未安装。'
    version=$(awk -F= 'NR==1 && $1=="version" && $2 ~ /^[0-9]+[.][0-9]+[.][0-9]+$/ {print $2}' "$work/checksums")
    [ -n "$version" ] || fail '发布版本无效。'
    [ -z "${SSHM_VERSION:-}" ] || [ "$version" = "$SSHM_VERSION" ] || fail '指定版本与签名清单不一致。'
    expected=$(awk 'NR==2 && length($1)==64 && $1 ~ /^[0-9a-f]+$/ && $2=="sshm-linux-x86_64.tar.gz" {print $1}' "$work/checksums")
    [ -n "$expected" ] && [ "$(wc -l < "$work/checksums" | tr -d ' ')" = 2 ] || fail '产物清单格式无效。'
    download sshm-linux-x86_64.tar.gz package.tar.gz 67108864
    actual=$(sha256sum "$work/package.tar.gz" | awk '{print $1}')
    [ "$actual" = "$expected" ] || fail '程序包摘要不匹配，原程序保持不变。'
    [ "$(tar -tzf "$work/package.tar.gz")" = sshm ] || fail '压缩包包含非预期文件。'
    # Extract only the named entry to stdout; never honor archive paths or links.
    tar -xOzf "$work/package.tar.gz" sshm | head -c 67108865 > "$work/sshm"
    [ "$(wc -c < "$work/sshm")" -le 67108864 ] || fail '解压后的程序超过大小限制。'
    chmod 755 "$work/sshm"
    reported=$("$work/sshm" --version) || fail '程序无法在本机启动；原程序未修改。'
    [ "$reported" = "sshm $version" ] || fail '程序版本与签名清单不一致。'
    binary_hash=$(sha256sum "$work/sshm" | awk '{print $1}')

    # Pass fixed code as an argument, not a mutable user-owned script to sudo.
    apply=$(apply_code)
    if [ "$install_dir" = /usr/local/bin ] && [ "$(id -u)" != 0 ]; then
        command -v sudo >/dev/null 2>&1 || fail '系统安装需要 sudo；也可设置 SSHM_INSTALL_DIR 为自己的 ~/.local/bin。'
        printf '将安装 sshm %s；接下来可能需要输入 sudo 密码。\n' "$version"
        sudo /bin/sh -c "$apply" sshm-install "$work/sshm" "$binary_hash" "$install_dir" || fail '安装未完成。'
    else
        /bin/sh -c "$apply" sshm-install "$work/sshm" "$binary_hash" "$install_dir" || fail '安装未完成。'
    fi
    printf '已安装 sshm %s → %s/sshm\n' "$version" "$install_dir"
    found=$(command -v sshm || true)
    if [ "$found" = "$install_dir/sshm" ]; then
        printf '现在输入 sshm 即可使用。\n'
    else
        printf '请确保 %s 位于 PATH 中且优先于其他 sshm，随后新开终端。\n' "$install_dir"
    fi
}
main "$@"
