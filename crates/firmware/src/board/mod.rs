//! 板级隔离：默认不驱动未确认的 GPIO，官方板和外接单键原型均须显式启用。

#[cfg(all(feature = "official-otg", feature = "prototype-button"))]
compile_error!("official-otg and prototype-button are mutually exclusive board configurations");

#[cfg(feature = "official-otg")]
mod official_otg;

#[cfg(feature = "official-otg")]
pub use official_otg::Board;

#[cfg(all(not(feature = "official-otg"), not(feature = "prototype-button")))]
mod generic;

#[cfg(all(not(feature = "official-otg"), not(feature = "prototype-button")))]
pub use generic::Board;

#[cfg(all(feature = "prototype-button", not(feature = "official-otg")))]
mod prototype_button;

#[cfg(all(feature = "prototype-button", not(feature = "official-otg")))]
pub use prototype_button::Board;
