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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustScanScope {
    Production,
    Test,
    All,
}

impl Default for RustScanScope {
    fn default() -> Self {
        Self::Production
    }
}

impl RustScanScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Test => "test",
            Self::All => "all",
        }
    }

    pub fn includes_tests(&self) -> bool {
        !matches!(self, Self::Production)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustFeatureSelection {
    Default,
    None,
    All,
    Selected {
        features: Vec<String>,
        no_default_features: bool,
    },
}

impl Default for RustFeatureSelection {
    fn default() -> Self {
        Self::Default
    }
}

impl RustFeatureSelection {
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::None => "none",
            Self::All => "all",
            Self::Selected { .. } => "selected",
        }
    }

    pub fn selected_features(&self) -> &[String] {
        match self {
            Self::Selected { features, .. } => features,
            _ => &[],
        }
    }

    pub fn no_default_features(&self) -> bool {
        match self {
            Self::None => true,
            Self::Selected {
                no_default_features,
                ..
            } => *no_default_features,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RustScanOptions {
    pub scope: RustScanScope,
    pub features: RustFeatureSelection,
}
