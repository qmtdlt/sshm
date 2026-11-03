#!/bin/sh
# Installed inside each root-owned version directory. Does not touch user data.
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
[ "$(id -u)" = 0 ] || { echo '请使用 sudo 运行。' >&2; exit 1; }
base=/usr/local/libexec/sshm
entry=/usr/local/bin/sshm
for dir in /usr/local /usr/local/bin /usr/local/libexec "$base" "$base/versions"; do
    [ -d "$dir" ] && [ ! -L "$dir" ] && [ "$(stat -f %u "$dir")" = 0 ] || exit 1
    mode=$(stat -f %Lp "$dir")
    [ $((0$mode & 0022)) = 0 ] || exit 1
done
[ -L "$entry" ] || { echo '入口不是受管链接，拒绝修改。' >&2; exit 1; }
current=$(readlink "$entry")
case "$current" in "$base"/versions/*/sshm) ;; *) exit 1;; esac
case "${1:-}" in
    rollback)
        previous=$(readlink "$base/previous")
        case "$previous" in "$base"/versions/*/sshm) ;; *) exit 1;; esac
        "$previous" --version
        stage=$(mktemp -d "$base/.rollback.XXXXXXXX")
        trap 'rm -rf "$stage"' EXIT
        ln -s "$previous" "$stage/current"
        mv -fh "$stage/current" "$entry"
        echo '已恢复上一版；用户数据未改动。'
        ;;
    uninstall)
        rm "$entry"
        rm -rf "$base"
        pkgutil --forget com.qmtdlt.sshm >/dev/null
        echo '程序已卸载；连接、Keychain 凭据、私钥和更新缓存均保留。'
        ;;
    *) echo '用法：manage rollback|uninstall' >&2; exit 2;;
esac
