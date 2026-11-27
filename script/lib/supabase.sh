# shellcheck shell=bash
#
# Shared by script/db-*, script/auth-up, script/api-dev, script/api-check and
# script/site-server (sourced, not run): the Supabase CLI for this
# repository's local stack, Auth's signing key, the stack's addresses, and the
# local database's connection strings. Every CLI call here is local; nothing
# talks to a hosted project.

# Usage data off. CLI 2.54 collects none; later releases read these two.
export SUPABASE_TELEMETRY_DISABLED=1
export DO_NOT_TRACK=1

root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
# The API role's password for the local database, generated on this machine.
api_password_file="$root/.state/devtab_api.password"

# Supabase Auth's signing key for access tokens (ES256), which
# supabase/config.toml names: made on first use, mode 600, in the ignored
# .state/, never committed. The CLI refuses to run without the file.
signing_keys_file="$root/.state/supabase/signing-keys.json"

ensure_signing_keys() {
  if [[ -s "$signing_keys_file" && "$(tr -d '[:space:]' <"$signing_keys_file")" != "[]" ]]; then
    return
  fi
  (umask 077 && mkdir -p "$(dirname "$signing_keys_file")" && printf '[]\n' >"$signing_keys_file")
  supabase --workdir "$root" gen signing-key --algorithm ES256 --append >/dev/null
}

supabase_cli() {
  if ! command -v supabase >/dev/null 2>&1; then
    echo "The Supabase CLI is not installed (README.md, \"Backend\")." >&2
    exit 1
  fi
  ensure_signing_keys
  supabase --workdir "$root" "$@"
}

# The local stack's API gateway (supabase/config.toml, [api] port): Auth is
# under /auth/v1, its JWKS at /auth/v1/.well-known/jwks.json.
local_supabase_url="http://127.0.0.1:54332"

# One value of `supabase status -o env` (PUBLISHABLE_KEY, MAILPIT_URL, ...),
# read when needed and never written into a file.
local_status_value() {
  supabase_cli status -o env 2>/dev/null | sed -n "s/^$1=\"\(.*\)\"$/\1/p"
}

# The owner's URL of the running local database, as the CLI reports it.
local_db_url() {
  local url
  url="$(supabase_cli status -o env 2>/dev/null | sed -n 's/^DB_URL="\(.*\)"$/\1/p')"
  if [[ -z "$url" ]]; then
    echo "The local database is not running. Start it with script/db-up." >&2
    exit 1
  fi
  printf '%s\n' "$url"
}

# Gives devtab_api the password from .state/, generated there on first use.
# The migrations create the role without one; a reset creates it again.
set_api_password() {
  local url password
  if ! command -v psql >/dev/null 2>&1; then
    echo "psql is not installed (README.md, \"Backend\")." >&2
    exit 1
  fi
  url="$(local_db_url)"
  if [[ ! -s "$api_password_file" ]]; then
    mkdir -p "$(dirname "$api_password_file")"
    (umask 077 && openssl rand -hex 24 >"$api_password_file")
  fi
  password="$(<"$api_password_file")"
  # On stdin, so that the password never appears in a process list.
  printf "do \$\$ begin
  if exists (select 1 from pg_roles where rolname = 'devtab_api') then
    alter role devtab_api password '%s';
  end if;
end \$\$;\n" "$password" | psql "$url" -X -q -v ON_ERROR_STOP=1
}

# The local database's URL for the API role.
api_db_url() {
  local url
  if [[ ! -s "$api_password_file" ]]; then
    echo "The API role has no local password yet. Run script/db-up." >&2
    exit 1
  fi
  url="$(local_db_url)"
  printf '%s\n' "$url" | sed "s#//[^@]*@#//devtab_api:$(<"$api_password_file")@#"
}
