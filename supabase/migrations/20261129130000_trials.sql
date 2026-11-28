-- A trial without a card (D36).
--
-- - A trial is a licence of kind `trial`: a plan's seats until `paid_until`,
--   with no customer and no provider, read by the token's rules like a
--   complimentary licence with an end (no payment grace, never renews).
-- - One trial per inbox, whatever its spelling: `app.trial_inboxes` keeps the
--   SHA-256 of the canonical inbox (`app.users.email_canonical`) of every
--   trial ever started, not the address, and outlives the account, so that
--   deleting the account and signing up again with `a.b+2@gmail.com` gives no
--   second trial.

alter table app.licences drop constraint licences_kind_check;
alter table app.licences add constraint licences_kind_check
  check (kind in ('subscription', 'one_time', 'complimentary', 'trial'));
alter table app.licences drop constraint licences_check1;
alter table app.licences add constraint licences_customer_check
  check (kind in ('complimentary', 'trial') or customer_id is not null);
alter table app.licences add constraint licences_trial_ends
  check (kind <> 'trial' or paid_until is not null);

create table app.trial_inboxes (
  -- SHA-256 of the canonical inbox; the address itself is not kept.
  inbox_hash bytea primary key check (octet_length(inbox_hash) = 32),
  licence_id uuid references app.licences (id) on delete set null,
  started_at timestamptz not null
);
alter table app.trial_inboxes enable row level security;
