#![deny(unsafe_code)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
pub mod api;
mod controller;
mod resource;

pub use controller::run;
pub use resource::{
    Ephemeron, EphemeronCondition, EphemeronService, EphemeronSpec, EphemeronStatus,
};
