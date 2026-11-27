-- Purchases: the checkouts a signed-in person starts, the payment provider's
-- events as they arrive, and the keys still to be shown once; plus when a
-- subscription's payment was first seen overdue.
--
-- A checkout belongs to the person who started it, and only that person
-- may see the key it led to. An event's body is kept as it came, for replay
-- and diagnosis; it may carry the buyer's e-mail address, so it is cleared
-- after 90 days. A key waiting to be shown is the only key ever stored in
-- plain text, until it is shown or a day has passed; the backups skip it.

-- When the API first saw the subscription's payment overdue (the API's
-- clock); null while it is paid. The provider moves its period ahead while
-- it retries, so the paid period stays where it was and this marks the
-- start of the grace.
alter table app.licences add column past_due_since timestamptz;

create table app.checkouts (
  -- Also the `ref` of the return address.
  id uuid primary key default gen_random_uuid(),
  -- Who started it; only they see the key. Deleting the account deletes it.
  user_id uuid not null references app.users (id) on delete cascade,
  plan_id text not null references app.plans (id),
  price_id uuid not null references app.plan_prices (id),
  provider text not null check (provider ~ '^[a-z0-9-]{2,32}$'),
  -- Set once the provider has started its side.
  provider_checkout_id text check (char_length(provider_checkout_id) between 1 and 255),
  -- A change of plan of this licence; null for a new purchase.
  for_licence_id uuid references app.licences (id) on delete set null,
  -- The licence it led to, once completed.
  licence_id uuid references app.licences (id) on delete set null,
  status text not null default 'open' check (status in ('open', 'completed', 'expired')),
  -- The language of the mail with the key.
  locale text not null check (locale in ('en', 'ru')),
  created_at timestamptz not null,
  completed_at timestamptz,
  expires_at timestamptz not null,
  unique (provider, provider_checkout_id),
  check ((status = 'completed') = (completed_at is not null)),
  check (expires_at > created_at)
);
create index checkouts_user_idx on app.checkouts (user_id, created_at);
create index checkouts_licence_idx on app.checkouts (licence_id);

-- Every event a provider delivered with a valid signature.
create table app.provider_events (
  id bigserial primary key,
  provider text not null check (provider ~ '^[a-z0-9-]{2,32}$'),
  -- The provider's id of the event: a repeated delivery is not stored twice.
  event_id text not null check (char_length(event_id) between 1 and 255),
  event_type text not null check (char_length(event_type) between 1 and 100),
  -- When the provider made it; for the record only, never an order.
  occurred_at timestamptz not null,
  received_at timestamptz not null,
  processed_at timestamptz,
  status text not null default 'received'
    check (status in ('received', 'processed', 'ignored', 'failed')),
  attempts integer not null default 0 check (attempts >= 0),
  -- Never personal data.
  last_error text check (char_length(last_error) <= 1000),
  -- The body as it came; personal data (the buyer's address): cleared after
  -- 90 days.
  payload jsonb,
  unique (provider, event_id)
);
create index provider_events_pending_idx on app.provider_events (status, received_at)
  where status in ('received', 'failed');

-- A key to show once to the buyer and to mail. Secret: never in a backup.
create table app.key_reveals (
  licence_id uuid primary key references app.licences (id) on delete cascade,
  key_plain text not null,
  created_at timestamptz not null,
  expires_at timestamptz not null,
  revealed_at timestamptz,
  mailed_at timestamptz,
  check (expires_at > created_at)
);

alter table app.checkouts enable row level security;
alter table app.provider_events enable row level security;
alter table app.key_reveals enable row level security;
