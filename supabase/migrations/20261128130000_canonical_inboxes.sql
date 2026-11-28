-- Each user's canonical inbox: the account's address with what mail
-- providers ignore taken away, so that one inbox under many spellings is
-- recognised as one. DevTab.Dev will offer a trial without a card, once per
-- inbox; `first.last+trial2@gmail.com` and `firstlast@googlemail.com` are the
-- same inbox and must not get a second one.
--
-- The rule (app.canonical_email, mirrored by the API's `canonical_email` and
-- held to it by a test): trimmed and lower case; on every domain a `+tag`
-- after the local part's first character is dropped; on gmail.com and
-- googlemail.com the dots of the local part are dropped and the domain is
-- gmail.com. The account's own address (Supabase Auth's `auth.users.email`)
-- stays as the person typed it: they sign in with it, and mail goes to it.
--
-- `app.users.email_canonical` is kept by a trigger on `auth.users` (a new
-- user, a changed address), so the API never reads `auth`. A paid guest
-- checkout finds its buyer by the exact address first, else by this; the
-- trial-once rule will key on it too.

create function app.canonical_email(p_email text) returns text
language plpgsql
immutable
parallel safe
set search_path = ''
as $$
declare
  v_email text := lower(btrim(p_email));
  v_at integer;
  v_local text;
  v_domain text;
  v_plus integer;
begin
  if v_email is null or v_email = '' then
    return null;
  end if;
  if position('@' in v_email) = 0 then
    return v_email;
  end if;
  -- The last `@`: what follows is the domain.
  v_at := length(v_email) - position('@' in reverse(v_email)) + 1;
  v_local := left(v_email, v_at - 1);
  v_domain := substr(v_email, v_at + 1);
  v_plus := position('+' in v_local);
  if v_plus > 1 then
    v_local := left(v_local, v_plus - 1);
  end if;
  if v_domain in ('gmail.com', 'googlemail.com') then
    v_local := replace(v_local, '.', '');
    v_domain := 'gmail.com';
  end if;
  return v_local || '@' || v_domain;
end;
$$;
revoke execute on function app.canonical_email(text) from public, anon, authenticated, service_role;
grant execute on function app.canonical_email(text) to devtab_api;

alter table app.users add column email_canonical text
  check (email_canonical = lower(email_canonical) and char_length(email_canonical) between 1 and 320);
create index users_email_canonical_idx on app.users (email_canonical);

-- The user's row with the canonical form of their address. An upsert, so
-- that it does not depend on the order of the triggers on `auth.users`.
create function app.on_auth_user_email() returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  insert into app.users (id, email_canonical) values (new.id, app.canonical_email(new.email))
  on conflict (id) do update set email_canonical = excluded.email_canonical;
  return new;
end;
$$;
revoke execute on function app.on_auth_user_email() from public, anon, authenticated, service_role;

create trigger app_on_auth_user_email after insert or update of email on auth.users
  for each row execute function app.on_auth_user_email();

-- Users who exist already.
update app.users u set email_canonical = app.canonical_email(a.email)
  from auth.users a where a.id = u.id;
