#!/bin/sh
# Package-owned closed WSL configuration helper. Every path is an argv value, never shell source.
set -eu
umask 077
action=${1-}
target=${2-}
case "$target" in /*) ;; *) exit 64;; esac
case "$target" in *'\n'*|*'\r'*|*\\*) exit 64;; esac
parent=${target%/*}
[ -n "$parent" ] && [ "$parent" != "$target" ] || exit 64
limit=1048576
case "$action" in
  read)
    [ -f "$target" ] || exit 66
    [ "$(wc -c < "$target")" -le "$limit" ] || exit 65
    cat -- "$target"
    ;;
  backup)
    backup=${3-}
    case "$backup" in /*) ;; *) exit 64;; esac
    case "$backup" in *'\n'*|*'\r'*|*\\*) exit 64;; esac
    backup_parent=${backup%/*}
    [ "$backup_parent" = "$parent" ] || exit 64
    [ -f "$target" ] && [ ! -e "$backup" ] || exit 65
    [ "$(wc -c < "$target")" -le "$limit" ] || exit 65
    ( set -C
      cat -- "$target" > "$backup"
      sync -f "$backup" 2>/dev/null || sync
    ) 2>/dev/null || exit 65
    ;;
  atomic-replace)
    mkdir -p -- "$parent"
    tmpdir=$(mktemp -d "$parent/.aiceland-config.XXXXXX") || exit 73
    tmp="$tmpdir/config"
    trap 'rm -rf -- "$tmpdir"' EXIT HUP INT TERM
    dd bs=65536 count=17 of="$tmp" status=none
    [ "$(wc -c < "$tmp")" -le "$limit" ] || exit 65
    sync -f "$tmp" 2>/dev/null || sync
    mv -f -- "$tmp" "$target"
    rmdir -- "$tmpdir"
    sync -f "$parent" 2>/dev/null || sync
    trap - EXIT HUP INT TERM
    ;;
  stage)
    mkdir -p -- "$parent"
    ( set -C
      dd bs=65536 count=17 status=none > "$target"
      [ "$(wc -c < "$target")" -le "$limit" ]
      sync -f "$target" 2>/dev/null || sync
    ) 2>/dev/null || exit 65
    ;;
  replace)
    staged=${3-}
    case "$staged" in /*) ;; *) exit 64;; esac
    case "$staged" in *'\n'*|*'\r'*|*\\*) exit 64;; esac
    staged_parent=${staged%/*}
    [ "$staged_parent" = "$parent" ] || exit 64
    [ -f "$staged" ] || exit 66
    mv -f -- "$staged" "$target"
    sync -f "$parent" 2>/dev/null || sync
    ;;
  *) exit 64;;
esac
