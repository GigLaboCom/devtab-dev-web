-- One row in app.users per Supabase Auth user (`auth.users`), with the same
-- id (the `sub` of a sign-in token). The e-mail address, sign-in identities
-- and sessions stay in Supabase Auth's tables; app keeps no copy of them.
--
-- A trigger on `auth.users` creates the row; the same idempotent function
-- runs once over the users that already exist. The API role cannot read
-- `auth`: the two lookups by e-mail address it needs (the key e-mail, the
-- admin commands) go through two security definer functions below.
-- Row level security is on with no policies, like every table in `app`.

create table app.users (
  id uuid primary key references auth.users (id) on delete cascade,
  status text not null default 'active' check (status in ('active', 'blocked')),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create trigger users_set_updated_at before update on app.users
  for each row execute function app.set_updated_at();

alter table app.users enable row level security;

-- Creates the user's row. Idempotent, so the trigger and the backfill share it.
create function app.provision_user(p_id uuid) returns void
language plpgsql
security definer
set search_path = ''
as $$
begin
  insert into app.users (id) values (p_id)
  on conflict (id) do nothing;
end;
$$;
revoke execute on function app.provision_user(uuid) from public, anon, authenticated, service_role;

create function app.on_auth_user_created() returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  perform app.provision_user(new.id);
  return new;
end;
$$;
revoke execute on function app.on_auth_user_created() from public, anon, authenticated, service_role;

create trigger app_on_auth_user_created after insert on auth.users
  for each row execute function app.on_auth_user_created();

-- Users who signed up before this migration.
do $$
begin
  perform app.provision_user(u.id) from auth.users u;
end
$$;

-- The e-mail address of a user, for the API role, which cannot read `auth`.
-- plpgsql, so that the body is checked when it runs rather than when it is
-- created: Supabase Auth owns `auth.users` and may change it.
create function app.user_email(p_id uuid) returns text
language plpgsql
stable
security definer
set search_path = ''
as $$
begin
  return (select u.email from auth.users u where u.id = p_id);
end;
$$;
revoke execute on function app.user_email(uuid) from public, anon, authenticated, service_role;
grant execute on function app.user_email(uuid) to devtab_api;

-- The user with this e-mail address (case and surrounding spaces ignored),
-- or null.
create function app.user_id_by_email(p_email text) returns uuid
language plpgsql
stable
security definer
set search_path = ''
as $$
begin
  return (
    select u.id from auth.users u
    where lower(u.email) = lower(btrim(p_email))
    order by u.created_at, u.id
    limit 1
  );
end;
$$;
revoke execute on function app.user_id_by_email(text) from public, anon, authenticated, service_role;
grant execute on function app.user_id_by_email(text) to devtab_api;
