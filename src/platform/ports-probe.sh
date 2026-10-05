# Metadata-only Unix probe. $1 is a numeric shell PID, never shell source.
case "$1" in ''|*[!0-9]*) exit 1;; esac
[ "$1" -gt 1 ] || exit 1
# Bound the tree by depth and size, and read only PID/PPID columns.
pids=$(ps -e -o pid= -o ppid= | head -n 65536 | awk -v root="$1" '
{ parent[$1]=$2 }
END { selected[root]=1; count=1; for (depth=0; depth<64; depth++) {
    before=count; for (pid in parent) if ((parent[pid] in selected) && !(pid in selected) && count<2048) { selected[pid]=1; count++ }
    if (count==before) break
} for (pid in selected) print pid }')
if [ -r "/proc/$1/net/tcp" ]; then
    # /proc prints address words in the remote kernel's native byte order.
    case "$(printf '\001\000\000\000' | od -An -tu4 | tr -d ' ')" in
        1) printf 'endian little\n';;
        16777216) printf 'endian big\n';;
        *) exit 1;;
    esac
    # Socket ownership comes from fds; /proc/net alone includes other panes.
    {
        count=0
        for pid in $pids; do
            for fd in /proc/"$pid"/fd/*; do
                count=$((count+1)); [ "$count" -le 8192 ] || break 2
                link=$(readlink "$fd" 2>/dev/null) || continue
                case "$link" in socket:\[*\]) inode=${link#socket:[}; printf '%s\n' "${inode%]}";; esac
            done
        done
    } | awk 'FILENAME=="-" { owned[$1]=1; next } $4=="0A" && owned[$10] { print "tcp " $2 }' - "/proc/$1/net/tcp" "/proc/$1/net/tcp6" | head -n 32
elif command -v lsof >/dev/null 2>&1; then
    list=$(printf '%s\n' "$pids" | tr '\n' ',' | sed 's/,$//')
    lsof -a -nP -iTCP -sTCP:LISTEN -p "$list" -Fnt 2>/dev/null | awk '
        /^t/ { family=substr($0,2) }
        /^n/ { address=substr($0,2); if (family=="IPv6") sub(/^\*:/,"[::]:",address); print "endpoint " address }
    ' | head -n 32
else
    exit 1
fi
