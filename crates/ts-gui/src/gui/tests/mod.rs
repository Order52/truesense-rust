//! Tests of the window server over real loopback sockets, with the application layer on the
//! firmware simulator. Nothing here starts a browser or touches the real machine.
#![allow(clippy::expect_used)]

mod bus;
mod contract;
mod harness;
mod run;
mod security;
mod stream;
