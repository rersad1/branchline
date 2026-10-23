# Supplied as an inherited rcfile pipe, never installed in HOME.
[[ -r ~/.bashrc ]] && source ~/.bashrc
[[ ${BRANCHLINE_SHELL_INTEGRATION:-1} == 0 ]] && return
# .bashrc normally loads this itself. Respect installations without completion.
if ! declare -F _completion_loader >/dev/null && [[ -r /usr/share/bash-completion/bash_completion ]]; then
    source /usr/share/bash-completion/bash_completion
fi
__branchline_user_ps1=$PS1
__branchline_compact=0
# Leave customized rcfiles/prompts alone. Stock Ubuntu rcfile gets the compact prompt.
if [[ ${BRANCHLINE_PROMPT:-} == compact ]] || { [[ ${BRANCHLINE_PROMPT:-} != user ]] && { [[ ! -e ~/.bashrc ]] || cmp -s ~/.bashrc /etc/skel/.bashrc; }; }; then
    __branchline_compact=1
fi
branchline_prompt() {
    case $1 in
        compact) __branchline_compact=1 ;;
        user) __branchline_compact=0; PS1=$__branchline_user_ps1 ;;
        *) printf '%s\n' 'Usage: branchline_prompt compact|user' ;;
    esac
}
# Prepend to scalar/array PROMPT_COMMAND, preserving command status for existing hooks.
__branchline_saved_pc=()
[[ -v PROMPT_COMMAND ]] && __branchline_saved_pc=("${PROMPT_COMMAND[@]}")
__branchline_cwd() {
    local LC_ALL=C encoded='' char hex path=$PWD
    while [[ -n $path ]]; do
        char=${path:0:1}; path=${path:1}
        printf -v hex '%%%02X' "'$char"
        encoded+=$hex
    done
    printf '\e]633;P;Cwd=%s\a' "$encoded"
}
__branchline_prompt() {
    local status=$?
    __branchline_cwd
    if (( __branchline_compact )); then
        # PS1 references data via parameter expansion, never evaluates path as shell code.
        local leaf=${PWD##*/}
        [[ -n $leaf ]] || leaf=/
        __branchline_leaf=${leaf//[[:cntrl:]]/?}
        if (( ${#__branchline_leaf} > 40 )); then __branchline_leaf="…${__branchline_leaf: -39}"; fi
        __branchline_failure=''
        (( status )) && __branchline_failure=" ! $status"
        PS1='\[\e[34m\]${__branchline_leaf}\[\e[31m\]${__branchline_failure}\[\e[0m\] \$ '
    fi
    return "$status"
}
branchline_integration_off() {
    PROMPT_COMMAND=("${__branchline_saved_pc[@]}")
    PS1=$__branchline_user_ps1
    printf '\e]633;P;Cwd=\a'
}
PROMPT_COMMAND=(__branchline_prompt "${__branchline_saved_pc[@]}")
# Readline handles input, completion, search, and bracketed paste. Keep user .inputrc.
# Add reverse completion only when unbound; do not change the user's editing mode.
__branchline_bindings=$(bind -p 2>/dev/null)
if [[ $__branchline_bindings != *'"\e[Z": '* ]]; then
    bind '"\e[Z": menu-complete-backward' 2>/dev/null
fi
unset __branchline_bindings
