#![cfg_attr(test, allow(deprecated))]

pub mod api;
mod dev_api;
pub mod dev_skills;
pub mod owner_api;
pub mod skill_release;
pub mod tenant_skills;

#[cfg(test)]
mod skill_release_tests;
