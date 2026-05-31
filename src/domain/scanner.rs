use anyhow::Result;
use std::path::Path;

use super::ast::{CallInfo, FileScan, LiveDependency, ScanResult};

/// A trait that defines how to parse Rust source files into domain intelligence artifacts
/// mapped into the `axon` graph-based boundary system.
///
/// The production implementation is `RustSynScanner`.
pub trait AstScanner {
    /// Extracts module/package dependencies (e.g. `use`, `import`) to build the cross-cutting graph.
    fn extract_live_dependencies(
        &self,
        file_path: &Path,
        source_code: &str,
    ) -> Result<Vec<LiveDependency>>;

    /// Parses the file to find types (classes/structs), enums, modules, and their behaviors (methods/functions)
    fn scan_file(&self, file_path: &Path, source_code: &str) -> Result<ScanResult>;

    /// Extracts function/method call edges from the source code.
    fn extract_calls(&self, file_path: &Path, source_code: &str) -> Result<Vec<CallInfo>>;

    /// Parse the file **once** and extract symbols, imports, and calls together.
    ///
    /// Prefer this over calling [`Self::scan_file`], [`Self::extract_live_dependencies`],
    /// and [`Self::extract_calls`] separately — each of those re-parses the file,
    /// so the combined extractor is roughly 3× cheaper on the dominant parse cost.
    fn scan_source(&self, file_path: &Path, source_code: &str) -> Result<FileScan>;
}
