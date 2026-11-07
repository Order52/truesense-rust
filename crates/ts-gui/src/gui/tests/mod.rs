//! Tests of the window server over real loopback sockets, with the application layer on the
//! firmware simulator. Nothing here starts a browser or touches the real machine.
#![allow(clippy::expect_used)]

mod alerts;
mod bus;
mod contract;
mod favourites;
mod harness;
mod info;
mod modebind;
mod run;
mod security;
mod stream;
