//! The registered session replay target: the contract double cases plus the
//! session provider test modules reserved by the contract landing.

#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

#[path = "session_replay/contract_double.rs"]
mod contract_double;
#[path = "session_replay/io.rs"]
mod io;
#[path = "session_replay/login.rs"]
mod login;
#[path = "session_replay/mirror.rs"]
mod mirror;
