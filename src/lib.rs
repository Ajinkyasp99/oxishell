//! oxishell library. Everything except `ui` is GTK-free and unit-tested.
pub const APP_ID: &str = "dev.oxishell.Oxishell";

pub mod agents;
pub mod config;
pub mod editor;
pub mod history;
pub mod ipc;
pub mod meta;
pub mod model;
pub mod notify;
pub mod paths;
pub mod session;
pub mod shellint;
pub mod sysstat;
pub mod theme;
mod theme_builtin;
pub mod ui;
pub mod update_check;
pub mod usage;
pub mod usage_alerts;
