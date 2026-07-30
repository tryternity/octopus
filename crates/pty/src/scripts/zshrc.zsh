# octopus-shell-integration (zshrc)
#
# Emits OSC 133 A/B/C/D (prompt-start / prompt-end / pre-exec /
# command-done-with-exit-code) so the host can detect command boundaries and
# agent CLI invocations. `status` is a read-only special in zsh, so we shadow $?
# into `_octopus_ret`.

{
  _octopus_user_zdotdir="${OCTOPUS_USER_ZDOTDIR:-$HOME}"
  [ -f "$_octopus_user_zdotdir/.zshrc" ] && source "$_octopus_user_zdotdir/.zshrc"
  unset _octopus_user_zdotdir
}

# Re-source guard within a single shell (e.g. user runs `source ~/.zshrc`).
# This is NOT exported, so each nested zsh installs its own hooks — desired,
# since every interactive shell needs its own prompt integration.
if [[ -z "$__OCTOPUS_HOOKS_LOADED" ]]; then
  __OCTOPUS_HOOKS_LOADED=1
  autoload -Uz add-zsh-hook 2>/dev/null

  _octopus_precmd() {
    local _octopus_ret=$?
    printf '\e]133;D;%s\e\\' "$_octopus_ret"
    # Re-inject prompt-end marker in case a framework rebuilt PS1 (p10k, starship).
    if [[ "$PS1" != *$'\e]133;B\e\\'* ]]; then
      PS1=$'%{\e]133;B\e\\%}'"$PS1"
    fi
    printf '\e]133;A\e\\'
  }

  _octopus_preexec() {
    local cmd="${1//[[:cntrl:]]/ }"
    printf '\e]133;C;%s\e\\' "${cmd[1,256]}"
  }

  if (( $+functions[add-zsh-hook] )); then
    add-zsh-hook precmd _octopus_precmd
    add-zsh-hook preexec _octopus_preexec
  fi

  # Warp/iTerm2-style word-end navigation: zsh's default `forward-word` (M-f /
  # Option+Right) overshoots to the START of the next word; `emacs-forward-word`
  # stops at the END of the current word, which is what nearly every other shell
  # and GUI editor does. Only rebind when the binding is still the stock zsh
  # default — respects any explicit remap in the user's .zshrc.
  if (( $+widgets[emacs-forward-word] )) \
     && [[ "$(bindkey '\ef')" == '"^[f" forward-word' ]]; then
    bindkey '\ef' emacs-forward-word
  fi

  _octopus_precmd
fi
:
