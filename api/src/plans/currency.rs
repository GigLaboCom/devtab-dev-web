//! Which currency a price may use. A real price uses a code ISO 4217 lists
//! and a browser's `Intl.supportedValuesOf('currency')` knows: the extension
//! skips a plan in any other. XTS, the code ISO 4217 reserves for tests, is
//! the fake provider's alone, and is served only in `dev`, as only the
//! extension's test and development builds accept it (`testCurrencies`).

/// The test currency of every placeholder price.
pub const TEST: &str = "XTS";

/// The codes `Intl.supportedValuesOf('currency')` lists in Node 24 (ICU 78,
/// CLDR 48): 162 codes. A browser's list can be a few codes shorter; a price
/// should use a currency its checkout and every supported browser know.
pub const REAL: [&str; 162] = [
    "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT",
    "BGN", "BHD", "BIF", "BMD", "BND", "BOB", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD", "CAD",
    "CDF", "CHF", "CLP", "CNY", "COP", "CRC", "CUC", "CUP", "CVE", "CZK", "DJF", "DKK", "DOP",
    "DZD", "EGP", "ERN", "ETB", "EUR", "FJD", "FKP", "GBP", "GEL", "GHS", "GIP", "GMD", "GNF",
    "GTQ", "GYD", "HKD", "HNL", "HRK", "HTG", "HUF", "IDR", "ILS", "INR", "IQD", "IRR", "ISK",
    "JMD", "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRW", "KWD", "KYD", "KZT", "LAK",
    "LBP", "LKR", "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD", "MMK", "MNT", "MOP", "MRU",
    "MUR", "MVR", "MWK", "MXN", "MYR", "MZN", "NAD", "NGN", "NIO", "NOK", "NPR", "NZD", "OMR",
    "PAB", "PEN", "PGK", "PHP", "PKR", "PLN", "PYG", "QAR", "RON", "RSD", "RUB", "RWF", "SAR",
    "SBD", "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SLL", "SOS", "SRD", "SSP", "STN", "SVC",
    "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TOP", "TRY", "TTD", "TWD", "TZS", "UAH", "UGX",
    "USD", "UYU", "UZS", "VES", "VND", "VUV", "WST", "XAF", "XCD", "XCG", "XDR", "XOF", "XPF",
    "XSU", "YER", "ZAR", "ZMW", "ZWG", "ZWL",
];

/// A real currency code.
pub fn is_real(code: &str) -> bool {
    REAL.binary_search(&code).is_ok()
}

/// Whether `provider` may state a price in `code`: a real currency, or the
/// test currency at the fake provider.
pub fn allowed_at(provider: &str, code: &str) -> bool {
    is_real(code) || (code == TEST && provider == crate::payments::fake::ID)
}

/// Whether a price in `code` may be served: a real currency always, the test
/// currency only when `test_currencies` (`dev`).
pub fn servable(code: &str, test_currencies: bool) -> bool {
    is_real(code) || (test_currencies && code == TEST)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_sorted_unique_and_holds_no_test_code() {
        assert!(REAL.windows(2).all(|w| w[0] < w[1]));
        assert!(
            REAL.iter()
                .all(|c| c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()))
        );
        assert!(!is_real("XTS") && !is_real("XXX"));
        assert!(is_real("EUR") && is_real("JPY"));
    }

    #[test]
    fn the_test_currency_belongs_to_the_fake_provider_and_to_dev() {
        assert!(allowed_at("fake", "XTS"));
        assert!(!allowed_at("other", "XTS"));
        assert!(allowed_at("other", "EUR"));
        assert!(!allowed_at("fake", "ABC"));
        assert!(servable("XTS", true));
        assert!(!servable("XTS", false));
        assert!(servable("EUR", false));
    }
}
