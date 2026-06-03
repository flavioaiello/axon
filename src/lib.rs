#![recursion_limit = "256"]

pub const VERSION: &str = match option_env!("AXON_VERSION") {
    Some(version) => version,
    None => "main (commit unknown)",
};

pub const BUILD_COMMIT: &str = match option_env!("AXON_BUILD_COMMIT") {
    Some(commit) => commit,
    None => "unknown",
};

pub mod domain;
pub mod mcp;
pub mod reasoning;
pub mod server;
pub mod store;
