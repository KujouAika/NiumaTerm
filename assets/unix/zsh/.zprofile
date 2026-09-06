# NiumaTerm zsh integration — startup-file forwarder. See `.zshenv`.

if [[ -f "$NMT_USER_ZDOTDIR/.zprofile" ]]; then
  ZDOTDIR="$NMT_USER_ZDOTDIR"
  source "$NMT_USER_ZDOTDIR/.zprofile"
  ZDOTDIR="$NMT_ZDOTDIR"
fi
