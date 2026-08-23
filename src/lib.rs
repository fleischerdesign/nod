//! `nod` — Universal Nix Orchestration & Deployment Engine.
//!
//! This library crate exposes the modules that back the `nod` CLI. The
//! executable entry point lives in [`main.rs`](../src/main.rs) and consumes
//! this API.
//!
//! A number of `pub` items exist as API stubs for future commands and are not
//! yet exercised end-to-end; this is intentionally left permissive.

#![allow(dead_code)]

pub mod application;
pub mod commands;
pub mod config;
pub mod domain;
pub mod infrastructure;
pub mod telemetry;
pub mod ui;