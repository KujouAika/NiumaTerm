# NiumaTerm shell integration for bash: FinalTerm OSC 133 marks.
#
# The terminal reads these to bound the prompt (`;A` -> `;B`), the echoed
# command line (`;B` -> `;C`) and the command's output (`;C` -> `;D <exit>`).
# An ordered A->B->C->D lifecycle is what earns boundary trust, which is what
# turns a session into command blocks with a fixed prompt dock; without the
# marks the terminal falls back to heuristic prompt sniffing.
#
# Written for bash 3.2, which is the version macOS ships.
#
# Deliberately absent, unlike the PowerShell integration: no screen clear at
# the block boundary and no `?1049l` alternate-screen recovery. The terminal
# clears its own grid when it freezes a block, and there is no second host-side
# grid here to keep in step. Leaving the alternate screen would be actively
# wrong under job control: a suspended full-screen program is sitting at a
# prompt with the alternate screen still its own, and `fg` must find it intact.

case "$-" in
  *i*) ;;
  *) return 0 ;;
esac

if [ -n "$NMT_BASH_INTEGRATION_LOADED" ]; then
  return 0
fi
NMT_BASH_INTEGRATION_LOADED=1

# Set before the DEBUG trap is installed so the rest of this file, and the
# first prompt's own hooks, are not mistaken for a user command.
__nmt_in_prompt=1

# The terminal strips `file://<host>` and takes the remainder verbatim, so the
# path travels unencoded — percent-encoding would arrive literal.
__nmt_report_cwd() {
  printf '\033]7;file://%s%s\007' "$HOSTNAME" "$PWD"
}

# Runs first in PROMPT_COMMAND: `$?` is the finished command's status and any
# other hook would overwrite it.
__nmt_precmd() {
  local exit_code=$?

  __nmt_in_prompt=1

  if [ -z "$__nmt_primed" ]; then
    __nmt_primed=1
    # An empty A->B->C cycle, so the `;D` below completes an ordered lifecycle
    # and boundary trust is granted at this first prompt instead of after the
    # first command.
    printf '\033]133;A\007\033]133;B\007\033]133;C\007'
  fi

  # `;D` closes the previous command's output region carrying its status,
  # `;A` opens the prompt.
  printf '\033]133;D;%s\007' "$exit_code"
  __nmt_report_cwd
  printf '\033]133;A\007'
}

__nmt_prompt_end_mark='\[\033]133;B\007\]'

# Runs last in PROMPT_COMMAND: a prompt framework rebuilds PS1 from its own
# hook, so the mark is re-applied per prompt rather than appended once. `\[\]`
# tells bash the sequence occupies no columns, keeping the prompt's width
# arithmetic correct.
__nmt_prompt_ready() {
  # Any earlier copy is stripped first: a framework that rebuilt PS1 around one
  # would otherwise leave it stranded mid-prompt, ending the prompt region
  # before the prompt does.
  PS1="${PS1//"$__nmt_prompt_end_mark"/}$__nmt_prompt_end_mark"

  # The prompt is drawn after this hook, so anything the user runs next is
  # theirs.
  __nmt_in_prompt=
}

# `;C` — command input ends, its output begins. DEBUG fires before every simple
# command, including the ones the prompt hooks and the command's own pipeline
# run, so only the first one after a prompt is the user's.
__nmt_preexec() {
  if [ -n "$__nmt_in_prompt" ]; then
    return
  fi
  __nmt_in_prompt=1
  printf '\033]133;C\007'
}

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

PROMPT_COMMAND="__nmt_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __nmt_prompt_ready"

trap '__nmt_preexec' DEBUG
