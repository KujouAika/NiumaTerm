# NiumaTerm bash integration — the file `--rcfile` names.
#
# `--rcfile` replaces the one file bash would otherwise have read, so this
# stands in for the user's whole startup sequence and has to replay it.
#
# Under the login hop the outer shell is launched with `--norc --noprofile` and
# does nothing but `exec` this one. That is deliberate: an `exec` carries the
# environment across but nothing else, so a function, alias or trap the profile
# chain defines would be lost if the chain ran out there. It runs here instead,
# in the shell the user actually gets.

if [ -n "$NMT_BASH_LOGIN" ]; then
  [ -r /etc/profile ] && . /etc/profile
  for __nmt_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
    if [ -r "$__nmt_profile" ]; then
      . "$__nmt_profile"
      break
    fi
  done
  unset __nmt_profile
else
  # `/etc/bash.bashrc` is only in the startup sequence when bash was compiled
  # with SYS_BASHRC, which Debian and its derivatives do and there is no
  # efficient way to test for. Sourcing it when it exists matches the systems
  # that ship one; systems without the file are unaffected.
  [ -r /etc/bash.bashrc ] && . /etc/bash.bashrc
  [ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"
fi
unset NMT_BASH_LOGIN

# Last, so its hooks are registered after everything the user's files install.
[ -r "$NMT_BASH_INTEGRATION" ] && . "$NMT_BASH_INTEGRATION"
unset NMT_BASH_INTEGRATION
