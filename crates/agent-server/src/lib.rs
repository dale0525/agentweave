#![cfg_attr(test, allow(deprecated))]

pub mod api;
mod dev_api;
pub mod dev_skills;
pub mod owner_api;
pub mod skill_release;
mod tenant_initialization;
pub mod tenant_skills;

#[cfg(test)]
mod skill_release_tests;
#[cfg(test)]
mod tenant_skills_tests;
