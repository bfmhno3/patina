//! Patina core: the only place that validates commands, appends events to the
//! log, and projects the log into views. Every front end (CLI, GUI, MCP) is a
//! thin adapter over this crate.
//!
//! Projections are pure functions of the event log and an explicit `now`;
//! nothing here reads the system clock or produces user-interface text.
#![forbid(unsafe_code)]
