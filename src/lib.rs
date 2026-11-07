//! cmux-linux library. Everything except `ui` is GTK-free and unit-tested.
pub const APP_ID: &str = "dev.cmux.Linux";

pub mod agents;
pub mod config;
pub mod ipc;
pub mod meta;
pub mod model;
pub mod notify;
pub mod paths;
pub mod session;
pub mod theme;
mod theme_builtin;
pub mod ui;
