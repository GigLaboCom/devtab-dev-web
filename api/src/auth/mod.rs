//! Signed-in people: Supabase Auth's access tokens, checked against the
//! project's JWKS (`token`), and the person of a request to the internal
//! listener (`user`). The site keeps the session; the API keeps none.

pub mod token;
pub mod user;

pub use token::{Claims, TokenError, Verifier};
pub use user::{CurrentUser, UserCache};
