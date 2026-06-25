//! `agentstack hook <shell>` — print a direnv-style shell hook (PLAN §10). When
//! you `cd` into a repo containing a `.agentstack` file (first line = profile
//! name), it activates that profile at project scope across your CLIs.
//!
//! Install: add `eval "$(agentstack hook zsh)"` to your shell rc.

use anyhow::Result;

use crate::cli::{HookArgs, Shell};

const ZSH: &str = r#"_agentstack_chpwd() {
  if [[ -f .agentstack ]]; then
    local prof; prof="$(head -n1 .agentstack 2>/dev/null)"
    [[ -n "$prof" ]] && agentstack use "$prof" --scope project --write >/dev/null 2>&1
  fi
}
typeset -ag chpwd_functions
[[ ${chpwd_functions[(r)_agentstack_chpwd]} == _agentstack_chpwd ]] || chpwd_functions+=(_agentstack_chpwd)
_agentstack_chpwd
"#;

const BASH: &str = r#"_agentstack_hook() {
  if [[ "$PWD" != "$_AGENTSTACK_LAST_DIR" ]]; then
    _AGENTSTACK_LAST_DIR="$PWD"
    if [[ -f .agentstack ]]; then
      local prof; prof="$(head -n1 .agentstack 2>/dev/null)"
      [[ -n "$prof" ]] && agentstack use "$prof" --scope project --write >/dev/null 2>&1
    fi
  fi
}
case "$PROMPT_COMMAND" in
  *_agentstack_hook*) ;;
  *) PROMPT_COMMAND="_agentstack_hook${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac
"#;

const FISH: &str = r#"function _agentstack_hook --on-variable PWD
  if test -f .agentstack
    set -l prof (head -n1 .agentstack 2>/dev/null)
    if test -n "$prof"
      agentstack use "$prof" --scope project --write >/dev/null 2>&1
    end
  end
end
"#;

pub fn run(args: &HookArgs) -> Result<()> {
    let snippet = match args.shell {
        Shell::Zsh => ZSH,
        Shell::Bash => BASH,
        Shell::Fish => FISH,
    };
    print!("{snippet}");
    Ok(())
}
