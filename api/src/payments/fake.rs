//! The fake provider: development and tests, never production. Its prices are
//! the ones the plans file gives it (amount and currency next to the price
//! id); they use XTS, the code ISO 4217 reserves for tests, so that no price
//! of development looks like a real one. `DEVTAB_PAYMENTS=fake` is refused
//! in `prod`, and a test currency is never served outside `dev`.

use std::collections::BTreeMap;

use super::{PaymentProvider, PriceSnapshot, ProviderError, ProviderFuture};

pub const ID: &str = "fake";

/// The fake provider with its price list.
#[derive(Debug, Clone, Default)]
pub struct FakeProvider {
    prices: BTreeMap<String, PriceSnapshot>,
}

impl FakeProvider {
    pub fn new(prices: impl IntoIterator<Item = (String, PriceSnapshot)>) -> Self {
        Self {
            prices: prices.into_iter().collect(),
        }
    }
}

impl PaymentProvider for FakeProvider {
    fn id(&self) -> &'static str {
        ID
    }

    fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot> {
        Box::pin(async move {
            self.prices
                .get(id)
                .cloned()
                .ok_or_else(|| ProviderError::NotFound(format!("price {id}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_the_prices_it_was_given() {
        let snapshot = PriceSnapshot {
            currency: "XTS".to_owned(),
            amount_minor: 1111,
        };
        let fake = FakeProvider::new([("pri_TEST_0001".to_owned(), snapshot.clone())]);
        assert_eq!(fake.id(), "fake");
        assert_eq!(fake.price("pri_TEST_0001").await.unwrap(), snapshot);
        assert!(matches!(
            fake.price("pri_TEST_0002").await,
            Err(ProviderError::NotFound(_))
        ));
    }
}
