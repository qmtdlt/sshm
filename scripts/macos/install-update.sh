#!/bin/sh
# Executed by sudo; arguments are package, expected SHA-256, pinned Apple Team ID.
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
[ "$(id -u)" = 0 ] || exit 1
source_package=$1
expected_hash=$2
team=$3
case "$expected_hash" in *[!a-fA-F0-9]*|'') exit 1;; esac
[ "${#expected_hash}" = 64 ] || exit 1
case "$team" in *[!A-Z0-9]*|'') exit 1;; esac
[ "${#team}" = 10 ] || exit 1
stage=$(mktemp -d /private/tmp/sshm-update.XXXXXXXX)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
cp "$source_package" "$stage/update.pkg"
actual=$(shasum -a 256 "$stage/update.pkg" | cut -d ' ' -f 1)
[ "$actual" = "$expected_hash" ] || { echo '安装包在授权期间发生变化，已停止。' >&2; exit 1; }
pkgutil --check-signature "$stage/update.pkg" > "$stage/signature.txt"
grep -E "Developer ID Installer: .*\($team\)$" "$stage/signature.txt" >/dev/null || { echo '安装包签名团队不匹配。' >&2; exit 1; }
spctl --assess --type install "$stage/update.pkg"
installer -pkg "$stage/update.pkg" -target /
