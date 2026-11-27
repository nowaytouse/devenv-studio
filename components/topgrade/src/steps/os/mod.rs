#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "linux")]
mod archlinux;
#[cfg(target_os = "dragonfly")]
pub mod dragonfly;
#[cfg(target_os = "freebsd")]
pub mod freebsd;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "openbsd")]
pub mod openbsd;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use windows::{reboot, shutdown};

#[cfg(unix)]
pub use unix::{reboot, shutdown};

// Exercise Windows manifest and retry decisions in the macOS test suite as well.
#[cfg(all(test, target_os = "macos"))]
#[path = "windows/dotnetup/recovery.rs"]
mod dotnetup_recovery;
#[cfg(all(test, target_os = "macos"))]
#[path = "windows/dotnetup/inventory.rs"]
mod inventory;
