# NiumaTerm zsh integration — startup-file forwarder. See `.zshenv`.
#
# The integration loads after the user's `.zshrc` so its prompt hooks are
# registered last: a prompt framework rebuilds PS1 on every prompt, and the
# hook that appends the prompt-end mark has to run after the one that rebuilt
# it.

if [[ -f "$NMT_USER_ZDOTDIR/.zshrc" ]]; then
  ZDOTDIR="$NMT_USER_ZDOTDIR"
  source "$NMT_USER_ZDOTDIR/.zshrc"
fi

source "$NMT_ZDOTDIR/nmt-integration.zsh"

# Hand ZDOTDIR back for good. zsh resolves `.zlogin` after this file, so the
# user's own reaches them without a forwarder, and child processes inherit the
# value they configured rather than ours. An unset ZDOTDIR arrives here as
# $HOME, which zsh treats identically.
ZDOTDIR="$NMT_USER_ZDOTDIR"
unset NMT_ZDOTDIR NMT_USER_ZDOTDIR
