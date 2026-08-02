# bash completion for shelfctl
#
# Subcommands and their VALUES are listed here rather than discovered, because
# shelfctl has no --list-commands and shelling out to parse --help on every tab
# press is both slow and fragile. The cost is that this file has to be updated
# when a subcommand is added — which is why the values below are the ones the
# source matches on, so a mismatch is greppable.
_shelfctl() {
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    # Value completion first: after --sort, only the sort keys are valid, and
    # offering subcommands there is worse than offering nothing.
    case "$prev" in
        --sort)   COMPREPLY=($(compgen -W "recent title author progress" -- "$cur")); return ;;
        --format) COMPREPLY=($(compgen -W "md json txt" -- "$cur")); return ;;
        --out)    COMPREPLY=($(compgen -f -- "$cur")); return ;;
    esac

    # The first word is a subcommand; everything after it is flags for that
    # subcommand. Offering `scan` again mid-line is noise.
    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=($(compgen -W "scan list export-highlights stats info help" -- "$cur"))
        return
    fi

    case "${COMP_WORDS[1]}" in
        list)              COMPREPLY=($(compgen -W "--sort --help" -- "$cur")) ;;
        export-highlights) COMPREPLY=($(compgen -W "--format --out --help" -- "$cur")) ;;
        info)              COMPREPLY=($(compgen -f -- "$cur")) ;;
        *)                 COMPREPLY=($(compgen -W "--help" -- "$cur")) ;;
    esac
}
complete -F _shelfctl shelfctl
