//! Integration tests: the real daemon on a temporary socket, driven through the real client and
//! the application layer, family by family. See [`rig`] for what is real and what is faked.

mod assembly;
mod fans;
mod misc;
mod power;
mod rig;
mod session;
mod settings;
mod undervolt;
mod wire;
