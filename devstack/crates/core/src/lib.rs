#[cfg(target_os = "macos")]
pub mod certs;
pub mod config;
pub mod dns;
pub mod error;
pub mod httpd;
pub mod mysql;
pub mod paths;
pub mod php;
pub mod provider;
pub mod service;
pub mod site;
#[cfg(target_os = "windows")]
pub mod vendor;
pub mod vhost;

#[cfg(target_os = "macos")]
pub mod elevate_macos;
#[cfg(target_os = "macos")]
pub mod setup;

#[cfg(target_os = "windows")]
pub mod elevate;
#[cfg(target_os = "windows")]
pub mod hosts;
#[cfg(target_os = "windows")]
pub mod httpd_win;
#[cfg(target_os = "windows")]
pub mod php_win;

#[cfg(target_os = "macos")]
pub mod vendor_mac;

pub use error::{Error, Result};
