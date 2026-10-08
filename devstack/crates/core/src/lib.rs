pub mod paths;
pub mod config;
pub mod site;
pub mod service;
pub mod vhost;
pub mod php;
pub mod dns;
pub mod httpd;
pub mod mysql;
pub mod provider;
pub mod vendor;
pub mod error;

#[cfg(target_os = "windows")]
pub mod hosts;
#[cfg(target_os = "windows")]
pub mod elevate;
#[cfg(target_os = "windows")]
pub mod php_win;
#[cfg(target_os = "windows")]
pub mod httpd_win;

pub use error::{Error, Result};
