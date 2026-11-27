-- A team plan's price may step down with the team's size (a volume-tiered
-- price at the provider: every member pays the step the whole team reaches).
-- `tiers` keeps the steps as the provider states them, in order:
-- [{"up_to": 4, "amount_minor": 600}, {"up_to": null, "amount_minor": 500}];
-- `amount_minor` stays the first step's, the price of the smallest team. Null
-- for a flat price, which every personal plan has.
alter table app.plan_prices add column tiers jsonb
  check (tiers is null or jsonb_typeof(tiers) = 'array');
