-- The plans a licence is issued on. `devtab-api admin plans sync` writes the
-- rows from config/plans.toml; no migration inserts any. Their texts and
-- prices are separate tables, added later. An id never changes once synced:
-- licences and the extension's plan cards refer to it.

create table app.plans (
  id text primary key check (id ~ '^[a-z0-9-]{1,32}$'),
  kind text not null check (kind in ('free', 'subscription', 'one_time')),
  -- The billing period of a subscription; `term` on the wire.
  "interval" text check ("interval" in ('month', 'year')),
  -- Installations per licence, copied onto a licence when it is issued.
  seats integer check (seats between 1 and 100),
  -- The order of the plan cards.
  sort smallint not null,
  -- Whether the plan is shown and sold now.
  available boolean not null default false,
  -- The end of a limited offer.
  until timestamptz,
  updated_at timestamptz not null default now(),
  check ((kind = 'subscription') = ("interval" is not null)),
  check (kind = 'free' or seats is not null)
);

create trigger plans_set_updated_at before update on app.plans
  for each row execute function app.set_updated_at();

alter table app.plans enable row level security;
