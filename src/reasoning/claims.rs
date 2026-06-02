pub(crate) const CLAIM_ARCHITECTURE_OVERVIEW: &str = "architecture.overview";
pub(crate) const CLAIM_CHECK_LAYER_VIOLATIONS: &str = "check.layer_violations";
pub(crate) const CLAIM_CHECK_CIRCULAR_DEPS: &str = "check.circular_deps";
pub(crate) const CLAIM_CHECK_AGGREGATE_QUALITY: &str = "check.aggregate_quality";
pub(crate) const CLAIM_CHECK_ORPHAN_CONTEXTS: &str = "check.orphan_contexts";
pub(crate) const CLAIM_CHECK_POLICY_VIOLATIONS: &str = "check.policy_violations";
pub(crate) const CLAIM_CHECK_DRIFT: &str = "check.drift";
pub(crate) const CLAIM_CHECK_ALL: &str = "check.all";
pub(crate) const CLAIM_WHY_LAYER_VIOLATIONS: &str = "why.layer_violations";
pub(crate) const CLAIM_WHY_CIRCULAR_DEPS: &str = "why.circular_deps";
pub(crate) const CLAIM_WHY_POLICY_VIOLATIONS: &str = "why.policy_violations";
pub(crate) const CLAIM_WHY_AGGREGATE_QUALITY: &str = "why.aggregate_quality";
pub(crate) const CLAIM_WHY_ORPHAN_CONTEXTS: &str = "why.orphan_contexts";
pub(crate) const CLAIM_DRIFT_OVERVIEW: &str = "drift.overview";
pub(crate) const CLAIM_DIAGNOSE_REFACTOR: &str = "diagnose.refactor";
pub(crate) const CLAIM_REFACTOR_PLAN: &str = "refactor.plan";
pub(crate) const CLAIM_SAFE_TO_DELETE_PREFIX: &str = "safe_to_delete";
pub(crate) const CLAIM_HOW_CONNECTED_PREFIX: &str = "how_connected";
pub(crate) const CLAIM_IMPACT_PREFIX: &str = "impact";
pub(crate) const CLAIM_HISTORY_PREFIX: &str = "history";
pub(crate) const CLAIM_SEARCH_PREFIX: &str = "search";

pub(crate) const CANONICAL_CLAIM_IDS: [&str; 16] = [
    CLAIM_ARCHITECTURE_OVERVIEW,
    CLAIM_CHECK_LAYER_VIOLATIONS,
    CLAIM_CHECK_CIRCULAR_DEPS,
    CLAIM_CHECK_AGGREGATE_QUALITY,
    CLAIM_CHECK_ORPHAN_CONTEXTS,
    CLAIM_CHECK_POLICY_VIOLATIONS,
    CLAIM_CHECK_DRIFT,
    CLAIM_CHECK_ALL,
    CLAIM_WHY_LAYER_VIOLATIONS,
    CLAIM_WHY_CIRCULAR_DEPS,
    CLAIM_WHY_POLICY_VIOLATIONS,
    CLAIM_WHY_AGGREGATE_QUALITY,
    CLAIM_WHY_ORPHAN_CONTEXTS,
    CLAIM_DRIFT_OVERVIEW,
    CLAIM_DIAGNOSE_REFACTOR,
    CLAIM_REFACTOR_PLAN,
];
