//! The storefront and the demo guide. Browsing works signed out (the catalog is compiled in);
//! stock, the cart and checkout need a session.

mod builder;
mod cart;
mod category;
mod checkout;
mod compare;
mod demo_guide;
mod home;
mod not_found;
mod product;
mod sign_in;

pub use builder::Builder;
pub use cart::CartPage;
pub use category::CategoryPage;
pub use checkout::Checkout;
pub use compare::Compare;
pub use demo_guide::DemoGuide;
pub use home::Home;
pub use not_found::NotFound;
pub use product::ProductPage;
pub use sign_in::SignIn;
