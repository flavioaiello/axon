//! Rust-native fact graph vocabulary.
//!
//! These are implementation-ground-truth facts extracted from source code and
//! persisted as graph relations. Semantic labels may refer to them, but do not
//! replace them as the primary model.

pub use super::ast::{
    CallInfo, CodeReference, ContextScan, DiscoveredDirective, DiscoveredEnum, DiscoveredFunction,
    DiscoveredMethod, DiscoveredModule, DiscoveredStruct, DiscoveredTrait, FileScan,
    LiveDependency, ScanResult,
};
pub use super::model::{ASTEdge, CallEdge, ImportEdge, ReferenceEdge, SourceFile, SymbolDef};
