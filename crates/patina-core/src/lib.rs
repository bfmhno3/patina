//! Patina core: the only place that validates commands, appends events to the
//! log, and projects the log into views. Every front end (CLI, GUI, MCP) is a
//! thin adapter over this crate.
//!
//! Projections are pure functions of the event log and an explicit `now`;
//! nothing here reads the system clock or produces user-interface text.
#![forbid(unsafe_code)]

pub mod calibration;
pub mod command;
pub mod dag;
pub mod engine;
pub mod error;
pub mod event;
pub mod gauges;
pub mod habit;
pub mod ids;
pub mod mode;
pub mod query;
pub mod recommend;
pub mod resolve;
pub mod review;
pub mod rhythm;
pub mod state;
pub mod store;
pub mod suggest;
#[cfg(test)]
mod testutil;
pub mod time;
pub mod timer;
pub mod visibility;
