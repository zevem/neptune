# Run by a POSIX shell on the SSH host, after authentication. $1 is an optional
# remote directory. Keep it separate from the shell program and fail visibly
# if it disappeared instead of silently opening a different directory.
if [ -n "$1" ]; then
    CDPATH= cd -- "$1" || exit
fi

# $2 and $3, when given, are this terminal's report credential and the script
# that installs the agent adapters for the shell below. A host where they
# cannot be installed gets its shell all the same.
if [ -n "$3" ]; then
    eval "$3"
fi

case "${SHELL##*/}" in
zsh)
    # A private, temporary startup directory adds hooks after the user's config.
    # No dotfiles are changed. Preserve their ZDOTDIR and normal login ordering.
    _neptune_dir=$(umask 077; mktemp -d "${TMPDIR:-/tmp}/neptune-zsh.XXXXXXXXXX") || exec "$SHELL" -il
    _neptune_user_zdotdir=${ZDOTDIR-$HOME}
    _neptune_user_zdotdir_set=${ZDOTDIR+x}
    _neptune_start_cwd=$1
    export _neptune_dir _neptune_user_zdotdir _neptune_user_zdotdir_set _neptune_start_cwd
    cat > "$_neptune_dir/.zshenv" <<'NEPTUNE_ZSHENV'
if [[ -n $_neptune_user_zdotdir_set ]]; then
    ZDOTDIR=$_neptune_user_zdotdir
else
    unset ZDOTDIR
fi
[[ -r ${ZDOTDIR-$HOME}/.zshenv ]] && source "${ZDOTDIR-$HOME}/.zshenv"
_neptune_user_zdotdir=${ZDOTDIR-$HOME}
_neptune_user_zdotdir_set=${ZDOTDIR+x}
ZDOTDIR=$_neptune_dir
NEPTUNE_ZSHENV
    cat > "$_neptune_dir/.zprofile" <<'NEPTUNE_ZPROFILE'
if [[ -n $_neptune_user_zdotdir_set ]]; then
    ZDOTDIR=$_neptune_user_zdotdir
else
    unset ZDOTDIR
fi
[[ -r ${ZDOTDIR-$HOME}/.zprofile ]] && source "${ZDOTDIR-$HOME}/.zprofile"
_neptune_user_zdotdir=${ZDOTDIR-$HOME}
_neptune_user_zdotdir_set=${ZDOTDIR+x}
ZDOTDIR=$_neptune_dir
NEPTUNE_ZPROFILE
    cat > "$_neptune_dir/.zshrc" <<'NEPTUNE_ZSHRC'
if [[ -n $_neptune_user_zdotdir_set ]]; then
    ZDOTDIR=$_neptune_user_zdotdir
else
    unset ZDOTDIR
fi
# Global zshrc (macOS /etc/zshrc) ran with the startup directory as ZDOTDIR.
[[ $HISTFILE == $_neptune_dir/* ]] && HISTFILE=${ZDOTDIR-$HOME}/${HISTFILE#$_neptune_dir/}
[[ -r ${ZDOTDIR-$HOME}/.zshrc ]] && source "${ZDOTDIR-$HOME}/.zshrc"
_neptune_user_zdotdir=${ZDOTDIR-$HOME}
_neptune_user_zdotdir_set=${ZDOTDIR+x}
ZDOTDIR=$_neptune_dir
NEPTUNE_ZSHRC
    cat > "$_neptune_dir/.zlogin" <<'NEPTUNE_ZLOGIN'
if [[ -n $_neptune_user_zdotdir_set ]]; then
    ZDOTDIR=$_neptune_user_zdotdir
else
    unset ZDOTDIR
fi
command rm -f -- "$_neptune_dir/.zshenv" "$_neptune_dir/.zprofile" "$_neptune_dir/.zshrc" "$_neptune_dir/.zlogin"
# Global compinit can create its cache before the user's ZDOTDIR is restored.
command rm -f -- "$_neptune_dir"/.zcompdump*(N)
command rmdir -- "$_neptune_dir"
unset _neptune_dir _neptune_user_zdotdir _neptune_user_zdotdir_set
[[ -r ${ZDOTDIR-$HOME}/.zlogin ]] && source "${ZDOTDIR-$HOME}/.zlogin"
# Login configuration may itself cd. Apply the requested directory last.
if [[ -n $_neptune_start_cwd ]]; then
    builtin cd -- "$_neptune_start_cwd" || exit
fi
unset _neptune_start_cwd
# The user's files have set the search path; the agent adapters lead it.
if [[ -n $NEPTUNE_AGENT_DIR ]]; then
    path=("$NEPTUNE_AGENT_DIR/bin" "${(@)path:#$NEPTUNE_AGENT_DIR/bin}")
fi

_neptune_report_cwd() {
    # Encode bytes so Unicode, percent signs and control characters round trip.
    local LC_ALL=C ch encoded= hex
    for ch in "${(@s::)PWD}"; do
        case "$ch" in
            [a-zA-Z0-9/._~-]) encoded+=$ch ;;
            *) printf -v hex '%%%02X' "'$ch"; encoded+=$hex ;;
        esac
    done
    printf '\033]7;file://neptune%s\007' "$encoded"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _neptune_report_cwd
add-zsh-hook chpwd _neptune_report_cwd
NEPTUNE_ZLOGIN
    ZDOTDIR=$_neptune_dir
    export ZDOTDIR
    exec "$SHELL" -il
    ;;
bash)
    # Bash reads its login files from a startup file of the adapters', which
    # then puts them first on the search path.
    if [ -n "$NEPTUNE_AGENT_DIR" ]; then
        exec "$SHELL" --rcfile "$NEPTUNE_AGENT_DIR/bashrc" -i
    fi
    exec "$SHELL" -il
    ;;
*)
    # Other shells can report OSC 7 using their existing integration.
    exec "${SHELL:-/bin/sh}" -il
    ;;
esac
