# NiumaTerm bash integration — rc wrapper.
#
# `--rcfile` replaces the file bash would otherwise have read, so whatever that
# was has to be sourced here before the integration is added; enabling the
# integration must not change which of the user's files run.
#
# Under the login hop the profile chain has already run and named its own rc,
# so `NMT_BASH_USER_RC` is unset there and only the integration is added.

if [ -n "$NMT_BASH_USER_RC" ] && [ -r "$NMT_BASH_USER_RC" ]; then
  . "$NMT_BASH_USER_RC"
fi

if [ -r "$NMT_BASH_INTEGRATION" ]; then
  . "$NMT_BASH_INTEGRATION"
fi

unset NMT_BASH_USER_RC NMT_BASH_INTEGRATION
