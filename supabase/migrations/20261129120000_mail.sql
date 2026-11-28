-- The mails the API sends (D37): each one is a `mail` job in the queue.
--
-- - A job may name a key that makes it once only (`dedupe_key`): one mail per
--   licence, kind and period, however many events ask for it. The key stays
--   once the job is done.
-- - The language a licence's mails are written in (`locale`): the page the
--   purchase was made on.
-- - A team member gets an id, so that a mail to them names the row, never
--   the address, in the queue.

alter table app.jobs add column dedupe_key text check (char_length(dedupe_key) between 1 and 300);
create unique index jobs_dedupe_key_idx on app.jobs (dedupe_key) where dedupe_key is not null;

alter table app.licences add column locale text check (locale in ('en', 'ru'));

alter table app.licence_members add column id uuid not null default gen_random_uuid();
create unique index licence_members_id_idx on app.licence_members (id);
