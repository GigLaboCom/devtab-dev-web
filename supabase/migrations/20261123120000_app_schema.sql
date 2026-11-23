-- Schema `app` holds every application table. Only the API role reads and
-- writes it; nobody else gets any privilege on it. It is not exposed through
-- Supabase's Data API (supabase/config.toml, [api] schemas), and the Data API
-- roles `anon`, `authenticated` and `service_role` get nothing in it either:
-- the browser's key is public, so every read and write goes through the API.
-- Migrations run as the database owner (`postgres`) and only ever go forward.

create schema if not exists app;

revoke all on schema app from public, anon, authenticated, service_role;
alter default privileges in schema app revoke all on tables from public, anon, authenticated, service_role;
alter default privileges in schema app revoke all on sequences from public, anon, authenticated, service_role;
-- A per-schema default cannot take away the global EXECUTE that public has on
-- new functions, so every function in `app` also revokes it itself.
alter default privileges in schema app revoke all on functions from public, anon, authenticated, service_role;

-- Shared trigger function for tables with an `updated_at` column.
create function app.set_updated_at() returns trigger
language plpgsql
set search_path = ''
as $$
begin
  new.updated_at := now();
  return new;
end;
$$;
revoke execute on function app.set_updated_at() from public, anon, authenticated, service_role;

-- The role the API connects as, directly to Postgres. Created without a
-- password: each environment sets its own outside the repository (locally
-- script/db-up generates one into the ignored .state/).
-- BYPASSRLS because the API is the only reader and writer and does its own
-- authorisation; row level security on every table stays a second line.
do $$
begin
  if not exists (select 1 from pg_roles where rolname = 'devtab_api') then
    create role devtab_api login bypassrls;
  end if;
  -- Postgres 16+ does not let the creator `set role` to a new role by default;
  -- the migration owner gets it so that tests and administrators can act as
  -- the API role inside a transaction.
  execute format('grant devtab_api to %I', current_user);
end
$$;

grant usage on schema app to devtab_api;
grant select, insert, update, delete on all tables in schema app to devtab_api;
grant usage, select on all sequences in schema app to devtab_api;
grant execute on function app.set_updated_at() to devtab_api;
alter default privileges in schema app grant select, insert, update, delete on tables to devtab_api;
alter default privileges in schema app grant usage, select on sequences to devtab_api;
