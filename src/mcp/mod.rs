pub mod prompts;
pub mod protocol;
pub mod resources;
mod router;
pub mod tools;
pub mod write_tools;

use crate::domain::model::DomainModel;
use crate::store::Store;

pub(crate) use router::{
    handle_global_request, handle_request_with_registry, parse_tool_call_params,
};

/// Load the implemented model from store, falling back to an empty model.
pub(crate) fn load_actual_model(store: &Store, workspace_path: &str) -> DomainModel {
    store
        .load_actual(workspace_path)
        .ok()
        .flatten()
        .unwrap_or_else(|| DomainModel::empty(workspace_path))
}
