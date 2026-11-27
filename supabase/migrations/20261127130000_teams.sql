-- Team licences: a plan sold per member, the licence's number of members,
-- the people a team's owner invited by e-mail, and which person connected an
-- installation from their account.
--
-- A team plan says its smallest team (`min_members`); a personal plan has
-- none. A licence counts its members (1 for a personal one) and gets the
-- plan's seats for each. The owner is the licence's user and the first member;
-- the others are invited by address and become themselves once they sign in
-- with it. Removing a member frees the seats of the installations they
-- connected from their account.

alter table app.plans add column min_members smallint
  check (min_members between 1 and 1000);
alter table app.plans add constraint plans_team_is_subscription
  check (min_members is null or kind = 'subscription');

alter table app.licences add column members integer not null default 1
  check (members between 1 and 1000);
-- A team gets the plan's seats for each member: more than a person's hundred.
alter table app.licences drop constraint licences_seats_check;
alter table app.licences add constraint licences_seats_check
  check (seats between 1 and 100000);

create table app.licence_members (
  licence_id uuid not null references app.licences (id) on delete cascade,
  -- Lower case, as the owner typed it; how the member signs in.
  email text not null check (email = lower(email) and char_length(email) between 3 and 254),
  -- Set when the member first connects from their account.
  user_id uuid references app.users (id) on delete set null,
  invited_at timestamptz not null,
  primary key (licence_id, email)
);
create index licence_members_email_idx on app.licence_members (email);
create index licence_members_user_idx on app.licence_members (user_id);

-- Who connected the installation from their account; null for one activated
-- with the key.
alter table app.installations add column user_id uuid references app.users (id) on delete set null;
create index installations_user_idx on app.installations (licence_id, user_id) where user_id is not null;

alter table app.licence_members enable row level security;
