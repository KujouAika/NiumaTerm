# NiumaTerm shell integration for zsh: FinalTerm OSC 133 marks.
#
# The terminal reads these to bound the prompt (`;A` -> `;B`), the echoed
# command line (`;B` -> `;C`) and the command's output (`;C` -> `;D <exit>`).
# An ordered A->B->C->D lifecycle is what earns boundary trust, which is what
# turns a session into command blocks with a fixed prompt dock; without the
# marks the terminal falls back to heuristic prompt sniffing.
#
# Deliberately absent, unlike the PowerShell integration: no screen clear at
# the block boundary and no `?1049l` alternate-screen recovery. The terminal
# clears its own grid when it freezes a block, and there is no second host-side
# grid here to keep in step. Leaving the alternate screen would be actively
# wrong under job control: a suspended full-screen program is sitting at a
# prompt with the alternate screen still its own, and `fg` must find it intact.

[[ -o interactive ]] || return 0

# A nested zsh inherits the user's own ZDOTDIR, so it normally never reaches
# this file; the guard covers a configuration that sources it a second time.
[[ -n "$NMT_ZSH_INTEGRATION" ]] && return 0
NMT_ZSH_INTEGRATION=1

autoload -Uz add-zsh-hook

# The terminal strips `file://<host>` and takes the remainder verbatim, so the
# path travels unencoded — percent-encoding would arrive literal.
__nmt_report_cwd() {
  printf '\033]7;file://%s%s\007' "$HOST" "$PWD"
}

__nmt_prompt_end_mark=$'%{\033]133;B\007%}'

__nmt_precmd() {
  # Read first: any later command overwrites $?.
  local exit_code=$?

  if [[ -z "$__nmt_primed" ]]; then
    __nmt_primed=1
    # An empty A->B->C cycle, so the `;D` below completes an ordered lifecycle
    # and boundary trust is granted at this first prompt instead of after the
    # first command. Emitted here rather than while `.zshrc` runs, where a
    # prompt framework's instant prompt would flag the output.
    printf '\033]133;A\007\033]133;B\007\033]133;C\007'
  elif [[ -z "$__nmt_command_started" ]]; then
    # An empty line, or one abandoned with Ctrl-C, runs no command, so
    # `preexec` never fired and the command region opened by the last `;B` is
    # still open. Close it here: a `;D` arriving straight after a `;B` is an
    # out-of-order lifecycle and costs the terminal its boundary trust.
    printf '\033]133;C\007'
  fi
  __nmt_command_started=

  # `;D` closes the previous command's output region carrying its status,
  # `;A` opens the prompt.
  printf '\033]133;D;%s\007' "$exit_code"
  __nmt_report_cwd
  printf '\033]133;A\007'

  # The prompt ends with `;B`. Re-applied per prompt rather than appended once
  # at load because a prompt framework rebuilds PS1 in its own `precmd`; this
  # hook is registered last, so it sees the final PS1 for this prompt. Any
  # earlier copy is stripped first: a framework that rebuilt PS1 around one
  # would otherwise leave it stranded mid-prompt, ending the prompt region
  # before the prompt does. `%{%}` tells zsh the sequence occupies no columns,
  # keeping the prompt's width arithmetic and right-prompt placement correct.
  PS1="${PS1//"$__nmt_prompt_end_mark"/}$__nmt_prompt_end_mark"
}

# `;C` — command input ends, its output begins.
__nmt_preexec() {
  __nmt_command_started=1
  printf '\033]133;C\007'
}

add-zsh-hook precmd __nmt_precmd
add-zsh-hook preexec __nmt_preexec

# The terminal keeps no scrollback of its own once blocks are authoritative, so
# a user clear is invisible to it unless announced in band. `;K` goes out
# before the erase, so the frozen blocks drop in step with the screen.
__nmt_announce_clear() {
  printf '\033]133;K\007'
}

clear() {
  __nmt_announce_clear
  command clear "$@"
}

__nmt_clear_screen() {
  __nmt_announce_clear
  zle .clear-screen
}

zle -N clear-screen __nmt_clear_screen
