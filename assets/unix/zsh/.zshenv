# NiumaTerm zsh integration — startup-file forwarder.
#
# The terminal points ZDOTDIR at the directory holding this file so it can
# append its OSC 133 hooks after the user's own configuration. zsh resolves
# every file of its startup series through ZDOTDIR, so each forwarder sources
# the user's counterpart with ZDOTDIR pointed back at their directory — a
# startup file that reads $ZDOTDIR must see their value, not ours — and then
# restores ours so zsh keeps finding the rest of the series here.
#
# `.zshrc` hands ZDOTDIR back for good, which is also how the user's own
# `.zlogin` is reached without a forwarder of its own.

: ${NMT_USER_ZDOTDIR:=$HOME}

if [[ -f "$NMT_USER_ZDOTDIR/.zshenv" ]]; then
  ZDOTDIR="$NMT_USER_ZDOTDIR"
  source "$NMT_USER_ZDOTDIR/.zshenv"
  # A .zshenv that sets ZDOTDIR itself is choosing where the rest of its own
  # series lives, so that choice becomes the target of the later forwarders.
  NMT_USER_ZDOTDIR="$ZDOTDIR"
  ZDOTDIR="$NMT_ZDOTDIR"
fi
