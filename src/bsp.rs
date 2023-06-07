//! Conditional reexporting of Board Support Packages.

#[cfg(not(any(feature = "bsp_rpi3", feature = "bsp_rpi4")))]
compile_error!("select exactly one board: bsp_rpi3 or bsp_rpi4");
#[cfg(all(feature = "bsp_rpi3", feature = "bsp_rpi4"))]
compile_error!("select exactly one board, not both");

#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4"))]
mod raspberrypi;

#[cfg(any(feature = "bsp_rpi3", feature = "bsp_rpi4"))]
pub use raspberrypi::*;
