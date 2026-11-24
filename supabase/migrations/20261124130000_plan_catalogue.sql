-- The plans' catalogue: the texts of a plan's card in each language, its
-- prices at the payment providers, and the catalogue's version. Like the plans
-- themselves, every row is written by `devtab-api admin plans sync` from
-- config/plans.toml; no migration inserts any. The extension's rules for a
-- catalogue (lengths in Unicode code points, at most eight lines of what a plan
-- includes, the test currency only at the fake provider) are checked by the
-- sync; the checks below are the second line.

-- The texts of a plan's card in one language. Every plan has English texts;
-- GET /v1/plans sends only those.
create table app.plan_texts (
  plan_id text not null references app.plans (id) on delete cascade,
  locale text not null check (locale in ('en', 'ru')),
  name text not null check (char_length(name) between 1 and 32),
  summary text check (char_length(summary) between 1 and 140),
  -- What the plan includes: a JSON array of 0 to 8 lines of 1 to 100
  -- characters, in the order to show.
  includes jsonb not null default '[]'
    check (jsonb_typeof(includes) = 'array' and jsonb_array_length(includes) <= 8),
  price_note text check (char_length(price_note) between 1 and 60),
  badge text check (char_length(badge) between 1 and 24),
  updated_at timestamptz not null default now(),
  primary key (plan_id, locale)
);

create trigger plan_texts_set_updated_at before update on app.plan_texts
  for each row execute function app.set_updated_at();

alter table app.plan_texts enable row level security;

-- A plan's price at a payment provider. The amount and the currency are the
-- provider's, read from it by its price id when the plans are synced (for the
-- fake provider, from the file); nobody types them in. A price that leaves
-- the file is deactivated, not deleted: purchases will refer to it.
create table app.plan_prices (
  id uuid primary key default gen_random_uuid(),
  plan_id text not null references app.plans (id),
  provider text not null check (provider ~ '^[a-z0-9-]{2,32}$'),
  provider_price_id text not null check (char_length(provider_price_id) between 1 and 255),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  -- In the currency's minor units; the extension accepts at most 100 000 000.
  amount_minor bigint not null check (amount_minor between 0 and 100000000),
  active boolean not null default true,
  created_at timestamptz not null default now(),
  unique (provider, provider_price_id),
  -- XTS, the code ISO 4217 reserves for tests, belongs to the fake provider
  -- alone.
  check (currency <> 'XTS' or provider = 'fake')
);

-- One active price per plan, provider and currency.
create unique index plan_prices_active on app.plan_prices (plan_id, provider, currency) where active;

alter table app.plan_prices enable row level security;

-- Small values of the application, one row each. `plans.catalog` is the
-- catalogue's version, `catalog` in GET /v1/plans: the date of the sync that
-- changed it and a sequence number of that day.
create table app.settings (
  key text primary key check (key ~ '^[a-z0-9._-]{1,64}$'),
  value text not null,
  updated_at timestamptz not null default now()
);

create trigger settings_set_updated_at before update on app.settings
  for each row execute function app.set_updated_at();

alter table app.settings enable row level security;
