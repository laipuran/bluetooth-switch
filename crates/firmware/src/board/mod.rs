//! 板级隔离：默认不驱动未确认的 GPIO，仅官方 OTG 板显式启用引脚配置。

#[cfg(feature = "official-otg")]
mod official_otg;

#[cfg(feature = "official-otg")]
pub use official_otg::Board;

#[cfg(not(feature = "official-otg"))]
mod generic;

#[cfg(not(feature = "official-otg"))]
pub use generic::Board;
