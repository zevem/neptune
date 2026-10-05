# Evaluated on the SSH host by the bootstrap, before the login shell starts,
# with this terminal's report credential in $2. Installs the agent adapters in
# the user's cache directory, once per version of this script, and names them
# for the shell. Nothing here reads or changes the user's configuration or an
# agent's: each adapter adds its hooks to one launch.
#
# An adapter reports through the terminal: `OSC 7717 ; neptune ; credential ;
# run ; ...`, which travels the connection like any output. It says which CLI
# opened or closed and what kind of moment a hook marks; prompts, commands and
# results stay on the host.
_neptune_install_agents() (
    umask 077
    dir=$1
    [ -n "$HOME" ] || exit 1
    [ "$(cat "$dir/stamp" 2>/dev/null)" = '@STAMP@' ] && exit 0
    mkdir -p "$dir/bin" || exit 1
    # Each file is replaced whole, so an agent already running keeps a
    # complete adapter.
    put() { cat > "$dir/$1.new" && mv -f "$dir/$1.new" "$dir/$1"; }

    put hook <<'NEPTUNE_EOF_HOOK' || exit 1
#!/bin/sh
# hook EVENT: run by an agent's hook with the event's input on stdin.
# Reports the event and the few words of its input that say what kind of
# moment it is, prints nothing and exits 0, so it decides nothing for the agent.
if [ -z "$NEPTUNE_AGENT_PASS" ] || [ -z "$NEPTUNE_AGENT_RUN" ]; then
    cat > /dev/null
    exit 0
fi
# Input carries tool results; the words wanted are near its start.
input=$(head -c 65536; cat > /dev/null)
# A quote inside a JSON string is escaped, so `"name":"` can only be a key.
field() {
    case $input in
    *"\"$1\":\""*) value=${input#*"\"$1\":\""} ;;
    *"\"$1\": \""*) value=${input#*"\"$1\": \""} ;;
    *) return 1 ;;
    esac
    value=${value%%\"*}
    case $value in
    '' | *[!A-Za-z0-9_.:-]*) return 1 ;;
    esac
    [ "${#value}" -le 128 ]
}
case $1 in
'' | *[!A-Za-z0-9_.-]*) exit 0 ;;
esac
line="hook;$1"
for name in tool_name notification_type permission_mode source trigger; do
    if field "$name"; then
        line="$line;$name=$value"
    fi
done
case $input in
*'"agent_id":"'* | *'"agent_id": "'*) line="$line;agent_id=1" ;;
esac
{ printf '\033]7717;neptune;%s;%s;%s\007' "$NEPTUNE_AGENT_PASS" "$NEPTUNE_AGENT_RUN" "$line" > /dev/tty; } 2> /dev/null
exit 0
NEPTUNE_EOF_HOOK

    put run <<'NEPTUNE_EOF_RUN' || exit 1
#!/bin/sh
# run AGENT [ARGUMENT...]: opens the CLI the adapter named AGENT stands for,
# with this launch's hooks, and reports that it opened and closed.
agent=$1
shift
dir=${0%/*}
# A CLI inside another does not speak for the one that is tracked.
unset NEPTUNE_AGENT_HOOK
real=
old=$IFS
IFS=:
for place in $PATH; do
    [ "$place" = "$dir/bin" ] && continue
    if [ -f "$place/$agent" ] && [ -x "$place/$agent" ]; then
        real=$place/$agent
        break
    fi
done
IFS=$old
if [ -z "$real" ]; then
    echo "$agent is not installed or is not on PATH" >&2
    exit 127
fi
# Untouched: a CLI inside another, a batch job, redirected input or output,
# and a terminal Neptune no longer listens to.
if [ -n "$NEPTUNE_AGENT_RUN" ] || [ -z "$NEPTUNE_AGENT_PASS" ] || [ ! -t 0 ] || [ ! -t 1 ]; then
    exec "$real" "$@"
fi
# `pi` is a name other programs have; one of those is no agent.
if [ "$agent" = pi ]; then
    case $(readlink -f "$real" 2> /dev/null) in
    *pi-coding-agent*) ;;
    *) exec "$real" "$@" ;;
    esac
fi
for argument in "$@"; do
    case "$agent:$argument" in
    @BATCH@) exec "$real" "$@" ;;
    esac
done
NEPTUNE_AGENT_RUN=$(od -An -N16 -tx1 /dev/urandom 2> /dev/null | tr -d ' \n')
[ -n "$NEPTUNE_AGENT_RUN" ] || NEPTUNE_AGENT_RUN=$$x$(date +%s)
NEPTUNE_AGENT_DIR=$dir
export NEPTUNE_AGENT_RUN NEPTUNE_AGENT_DIR
report() {
    { printf '\033]7717;neptune;%s;%s;%s\007' "$NEPTUNE_AGENT_PASS" "$NEPTUNE_AGENT_RUN" "$1" > /dev/tty; } 2> /dev/null
}
# A value as a TOML string; one that cannot be written leaves the CLI as it is.
toml() {
    case $1 in
    *[\"\\]* | *'
'*) return 1 ;;
    esac
    printf '"%s"' "$1"
}
case $agent in
claude)
    set -- --settings "$dir/claude.json" "$@"
    ;;
codex)
    # Hooks need this terminal's environment, not a shared daemon's; a Codex
    # without --no-daemon opens as it is.
    if "$real" --help 2> /dev/null | grep -q -e --no-daemon &&
        pass=$(toml "$NEPTUNE_AGENT_PASS") && place=$(toml "$dir"); then
        set -- --no-daemon @CODEX_HOOKS@ \
            -c "shell_environment_policy.set.NEPTUNE_AGENT_PASS=$pass" \
            -c "shell_environment_policy.set.NEPTUNE_AGENT_RUN=\"$NEPTUNE_AGENT_RUN\"" \
            -c "shell_environment_policy.set.NEPTUNE_AGENT_DIR=$place" "$@"
    fi
    ;;
opencode)
    # Neptune's plugin joins the user's own. --pure and its variable turn
    # every plugin off, and configuration already given for the launch is
    # the user's to keep.
    case " $* " in
    *" --pure "*) ;;
    *)
        if [ -z "$OPENCODE_PURE" ] && [ -z "$OPENCODE_CONFIG_CONTENT" ] && place=$(toml "$dir/opencode.js"); then
            OPENCODE_CONFIG_CONTENT="{\"plugin\":[$place]}"
            NEPTUNE_AGENT_HOOK='sh "$NEPTUNE_AGENT_DIR/hook"'
            export OPENCODE_CONFIG_CONTENT NEPTUNE_AGENT_HOOK
        fi
        ;;
    esac
    ;;
pi | omp)
    NEPTUNE_AGENT_HOOK='sh "$NEPTUNE_AGENT_DIR/hook"'
    export NEPTUNE_AGENT_HOOK
    set -- -e "$dir/pi.js" "$@"
    ;;
# Gemini CLI takes hooks only from its settings files, which are the user's.
# Its title says what it is doing.
gemini) ;;
esac
report "open;$agent"
# The CLI handles the terminal's interrupts; this shell waits for it.
trap : INT QUIT
"$real" "$@"
status=$?
report close
exit "$status"
NEPTUNE_EOF_RUN

    put claude.json <<'NEPTUNE_EOF_CLAUDE' || exit 1
@CLAUDE_SETTINGS@
NEPTUNE_EOF_CLAUDE
    put opencode.js <<'NEPTUNE_EOF_OPENCODE' || exit 1
@OPENCODE_PLUGIN@
NEPTUNE_EOF_OPENCODE
    put pi.js <<'NEPTUNE_EOF_PI' || exit 1
@PI_EXTENSION@
NEPTUNE_EOF_PI
    # Bash reads its login files, then puts the adapters first: a login
    # shell's own files may set the search path anew.
    put bashrc <<'NEPTUNE_EOF_BASHRC' || exit 1
[ -r /etc/profile ] && . /etc/profile
for _neptune_file in ~/.bash_profile ~/.bash_login ~/.profile; do
    if [ -r "$_neptune_file" ]; then
        . "$_neptune_file"
        break
    fi
done
unset _neptune_file
PATH=$NEPTUNE_AGENT_DIR/bin:$PATH
NEPTUNE_EOF_BASHRC

    for agent in @NAMES@; do
        printf '#!/bin/sh\ndir=${0%%/*}\nexec /bin/sh "${dir%%/*}/run" %s "$@"\n' "$agent" | put "bin/$agent" &&
            chmod 700 "$dir/bin/$agent" || exit 1
    done
    printf '%s\n' '@STAMP@' | put stamp
)
if [ -n "$2" ] && _neptune_install_agents "${XDG_CACHE_HOME:-$HOME/.cache}/neptune/agents"; then
    NEPTUNE_AGENT_DIR=${XDG_CACHE_HOME:-$HOME/.cache}/neptune/agents
    NEPTUNE_AGENT_PASS=$2
    # For shells that keep the search path they are given.
    PATH=$NEPTUNE_AGENT_DIR/bin:$PATH
    export NEPTUNE_AGENT_DIR NEPTUNE_AGENT_PASS PATH
fi
unset -f _neptune_install_agents
