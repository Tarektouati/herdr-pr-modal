//! herdr-pr-modal: a Herdr popup listing a repo's open PRs; selecting one
//! opens its head branch as a worktree-backed Herdr workspace.

pub mod app;
pub mod cache;
pub mod checkout;
pub mod cmd;
pub mod config;
pub mod context;
pub mod git;
pub mod herdr;
pub mod ids;
pub mod model;
pub mod provider;
pub mod session;
pub mod setup;
pub mod theme;
pub mod tui;
pub mod ui;
