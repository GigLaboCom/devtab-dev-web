# Shared by the hooks (sourced, not run): the private pattern list and the
# gitleaks binary.

common="$(git rev-parse --path-format=absolute --git-common-dir)"
root="$(git rev-parse --show-toplevel)"

# The private patterns, one extended regular expression per line, from
# <git common dir>/info/privacy-patterns. The file lives inside .git and is
# never committed.
private_patterns() {
  local list="$common/info/privacy-patterns"
  if [[ -f "$list" ]]; then grep -vE '^[[:space:]]*(#|$)' "$list" | awk '!seen[$0]++' || true; fi
}

# Prints the gitleaks binary: on PATH, or where Homebrew puts it (a hook run
# from a GUI client may not have Homebrew on PATH). Only one that runs counts.
find_gitleaks() {
  local bin
  for bin in "$(command -v gitleaks 2>/dev/null || true)" \
    /opt/homebrew/bin/gitleaks /usr/local/bin/gitleaks; do
    if [[ -n "$bin" && -x "$bin" ]] && "$bin" version >/dev/null 2>&1; then
      echo "$bin"
      return 0
    fi
  done
  return 1
}

gitleaks_missing() {
  echo "gitleaks is not installed. Install it (brew install gitleaks, or a"
  echo "release from https://github.com/gitleaks/gitleaks/releases with its"
  echo "checksums checked), or skip this hook deliberately with --no-verify."
}

# The patterns every hook refuses in added lines and messages, with a label
# each: absolute home paths, the global git address and the private list.
# The home-path patterns are split so that this file does not match itself.
load_patterns() {
  patterns=("/Us""ers/[^/[:space:]]+/" "/ho""me/[^/[:space:]]+/")
  labels=("an absolute macOS home path" "an absolute Linux home path")
  global_email="$(git config --global user.email || true)"
  if [[ -n "$global_email" ]]; then
    patterns+=("$(printf '%s' "$global_email" | sed 's/[][\.*^$()+?{}|]/\\&/g')")
    labels+=("the global git address")
  fi
  private=()
  local line
  while IFS= read -r line; do
    private+=("$line")
    patterns+=("$line")
    labels+=("private pattern ${#private[@]}")
  done < <(private_patterns)
}
