-- Licences, the installations of the extension that use them, the customers
-- of a payment provider who own them, the queue of side effects and the
-- record of administrators' actions.
--
-- Nothing here identifies a person beyond the user id: a licence key and an
-- installation secret are stored only as SHA-256 hashes, an installation id
-- is random and tied to neither a person nor a device, and no IP address is
-- kept anywhere. Row level security is on with no policies on every table.

-- A user as the customer of a payment provider. Created from a verified
-- provider event; the provider's customer id is a pseudonym.
create table app.customers (
  id uuid primary key default gen_random_uuid(),
  -- Null once the user's account is deleted.
  user_id uuid references app.users (id) on delete set null,
  provider text not null,
  provider_customer_id text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  -- When the account was deleted and the row lost its user.
  detached_at timestamptz,
  unique (user_id, provider),
  unique (provider, provider_customer_id)
);

create table app.licences (
  id uuid primary key default gen_random_uuid(),
  -- The licence id in tokens and in the extension's requests: 16 random
  -- bytes in Crockford base32, no time and no order in it.
  lid text not null unique check (lid ~ '^lic_[0-9A-HJKMNP-TV-Z]{26}$'),
  -- The owner; null for a complimentary licence issued to nobody, and after
  -- the owner's account is deleted.
  user_id uuid references app.users (id) on delete set null,
  customer_id uuid references app.customers (id),
  plan_id text not null references app.plans (id),
  kind text not null check (kind in ('subscription', 'one_time', 'complimentary')),
  status text not null default 'active' check (status in ('active', 'ended', 'revoked')),
  revoke_reason text check (revoke_reason in ('refund', 'key-blocked')),
  -- Installations allowed, copied from the plan when the licence is issued.
  seats integer not null check (seats between 1 and 100),
  -- A subscription: the end of the paid period. A complimentary licence: its
  -- end, or null for none. A one-time purchase: null.
  paid_until timestamptz,
  cancel_at_period_end boolean not null default false,
  provider text,
  provider_subscription_id text,
  provider_order_id text,
  -- SHA-256 of the canonical licence key; the key itself is never stored.
  key_hash bytea not null unique check (octet_length(key_hash) = 32),
  -- The key's last four characters, so that a person recognises it.
  key_hint text not null check (key_hint ~ '^[0-9A-HJKMNP-TV-Z]{4}$'),
  -- Grows each time the key is replaced.
  key_version integer not null default 1 check (key_version > 0),
  key_blocked_at timestamptz,
  -- An administrator's note; never personal data.
  note text check (char_length(note) <= 500),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  ended_at timestamptz,
  revoked_at timestamptz,
  check ((status = 'revoked') = (revoke_reason is not null)),
  check (kind = 'complimentary' or customer_id is not null),
  unique (provider, provider_subscription_id),
  unique (provider, provider_order_id)
);
create index licences_user_id_idx on app.licences (user_id);
create index licences_customer_id_idx on app.licences (customer_id);
create index licences_status_paid_until_idx on app.licences (status, paid_until);

-- One installation of the extension (one profile of one browser) in a
-- licence. `inst` is random, made by the extension.
create table app.installations (
  id uuid primary key default gen_random_uuid(),
  licence_id uuid not null references app.licences (id) on delete cascade,
  inst text not null check (inst ~ '^[A-Za-z0-9_-]{22}$'),
  -- SHA-256 of the installation's secret; the secret itself is never stored.
  secret_hash bytea not null check (octet_length(secret_hash) = 32),
  status text not null check (status in ('active', 'released', 'deactivated')),
  released_reason text check (released_reason in ('inactive', 'manual', 'admin')),
  -- The last time the installation took a seat.
  activated_at timestamptz not null,
  last_seen_at timestamptz not null,
  released_at timestamptz,
  unique (licence_id, inst),
  check ((status = 'released') = (released_reason is not null))
);
create index installations_active_licence_idx on app.installations (licence_id)
  where status = 'active';
create index installations_active_last_seen_idx on app.installations (last_seen_at)
  where status = 'active';

-- Side effects to run later (an outbox). The payload holds record ids only:
-- never an e-mail address and never a key.
create table app.jobs (
  id bigserial primary key,
  kind text not null,
  payload jsonb not null default '{}'::jsonb,
  run_at timestamptz not null default now(),
  attempts integer not null default 0,
  last_error text,
  done_at timestamptz
);
create index jobs_due_idx on app.jobs (run_at) where done_at is null;

-- Every change an administrator makes through `devtab-api admin`.
create table app.admin_audit (
  id bigserial primary key,
  at timestamptz not null default now(),
  -- The machine's user name the command ran as.
  actor text not null,
  action text not null,
  licence_id uuid references app.licences (id) on delete set null,
  -- Never personal data.
  details jsonb not null default '{}'::jsonb
);
create index admin_audit_licence_idx on app.admin_audit (licence_id, at);

create trigger customers_set_updated_at before update on app.customers
  for each row execute function app.set_updated_at();
create trigger licences_set_updated_at before update on app.licences
  for each row execute function app.set_updated_at();

alter table app.customers enable row level security;
alter table app.licences enable row level security;
alter table app.installations enable row level security;
alter table app.jobs enable row level security;
alter table app.admin_audit enable row level security;

-- The record of administrators' actions only grows.
revoke update, delete, truncate on app.admin_audit from devtab_api;
