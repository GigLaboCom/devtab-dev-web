-- Purchases without an account (a guest checkout): anyone may start a
-- checkout without signing in; the account comes from the payment. When the
-- provider reports the subscription paid, the API finds the Supabase Auth
-- user of the address the buyer paid with, or invites one, and the licence,
-- the checkout and the provider's customer go to that user.
--
-- - A guest checkout has no user until then. The browser that started it is
--   the only one that may see its key before the buyer's account does: it
--   holds a random secret in a cookie, and the checkout keeps only the
--   secret's SHA-256 (`claim_hash`).
-- - `buyer` records how the buyer got the account: `invited` (a new user,
--   invited by mail) or `existing` (the address had an account already, and
--   nobody is invited twice).
-- - A person may now be the provider's customer more than once: a guest
--   checkout cannot name the account's customer (nobody is signed in), so the
--   provider makes a new one with each guest purchase.

alter table app.checkouts alter column user_id drop not null;
alter table app.checkouts add column claim_hash bytea
  check (octet_length(claim_hash) = 32);
alter table app.checkouts add column buyer text
  check (buyer in ('invited', 'existing'));
alter table app.checkouts add constraint checkouts_user_or_claim
  check (user_id is not null or claim_hash is not null);
alter table app.checkouts add constraint checkouts_buyer_has_user
  check (buyer is null or user_id is not null);

alter table app.customers drop constraint customers_user_id_provider_key;
create index customers_user_idx on app.customers (user_id, provider);
