use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

use crate::domain::model::DomainModel;
use crate::store::Store;
use crate::store::cozo::{
    ACTUAL_STATE, ModelHealth, PersistedReasoningClaim, ReasoningAssumption, ReasoningDependency,
    ReasoningDerivation, ReasoningJustification, ReasoningProvenance, ReasoningSupportEdge,
    canonical_model_state, canonicalize_path,
};

mod claims;
use claims::*;

type CanonicalClaimBuilder = for<'a> fn(
    &ReasoningKernel<'a>,
    &str,
    &MaterializedReasoningData,
    i64,
) -> Result<PersistedReasoningClaim>;
type ParameterizedClaimBuilder = for<'a> fn(
    &ReasoningKernel<'a>,
    &str,
    &PersistedReasoningClaim,
    &MaterializedReasoningData,
    i64,
) -> Result<PersistedReasoningClaim>;

#[derive(Clone, Copy)]
struct CanonicalClaimRule {
    id: &'static str,
    build: CanonicalClaimBuilder,
}

#[derive(Clone, Copy)]
struct ParameterizedClaimRule {
    prefix: &'static str,
    build: ParameterizedClaimBuilder,
}

pub struct ReasoningKernel<'a> {
    store: &'a Store,
}

struct SnapshotBasis {
    desired_ts: i64,
    actual_ts: i64,
    drift_ts: i64,
}

struct MaterializedReasoningData {
    basis: SnapshotBasis,
    truth_assumptions: Vec<String>,
    layer_violations: Vec<(String, String, String)>,
    circular_deps: Vec<(String, String)>,
    aggregate_quality: Vec<(String, String)>,
    health: ModelHealth,
    policy_result: Value,
    pending_changes: Vec<Value>,
    drift_entries: Vec<Value>,
    has_actual: bool,
    ast_stats: Option<Value>,
}

impl<'a> ReasoningKernel<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn architecture(&self, workspace_path: &str) -> Result<PersistedReasoningClaim> {
        self.claim(workspace_path, CLAIM_ARCHITECTURE_OVERVIEW)
    }

    pub fn check(&self, workspace_path: &str, check_name: &str) -> Result<PersistedReasoningClaim> {
        let claim_id = match check_name {
            "layer_violations" => CLAIM_CHECK_LAYER_VIOLATIONS,
            "circular_deps" => CLAIM_CHECK_CIRCULAR_DEPS,
            "aggregate_quality" => CLAIM_CHECK_AGGREGATE_QUALITY,
            "orphan_contexts" => CLAIM_CHECK_ORPHAN_CONTEXTS,
            "policy_violations" => CLAIM_CHECK_POLICY_VIOLATIONS,
            "drift" => CLAIM_CHECK_DRIFT,
            "all" | "" => CLAIM_CHECK_ALL,
            other => anyhow::bail!("Unknown check '{other}'"),
        };
        self.claim(workspace_path, claim_id)
    }

    pub fn explain(
        &self,
        workspace_path: &str,
        violation_type: &str,
    ) -> Result<PersistedReasoningClaim> {
        let claim_id = match violation_type {
            "layer_violations" => CLAIM_WHY_LAYER_VIOLATIONS,
            "circular_deps" => CLAIM_WHY_CIRCULAR_DEPS,
            "policy_violations" => CLAIM_WHY_POLICY_VIOLATIONS,
            "aggregate_quality" => CLAIM_WHY_AGGREGATE_QUALITY,
            "orphan_contexts" => CLAIM_WHY_ORPHAN_CONTEXTS,
            other => anyhow::bail!("Unknown violation_type '{other}'"),
        };
        self.claim(workspace_path, claim_id)
    }

    pub fn drift(&self, workspace_path: &str) -> Result<PersistedReasoningClaim> {
        self.claim(workspace_path, CLAIM_DRIFT_OVERVIEW)
    }

    pub fn diagnose(&self, workspace_path: &str) -> Result<PersistedReasoningClaim> {
        self.claim(workspace_path, CLAIM_DIAGNOSE_REFACTOR)
    }

    pub fn refactor_plan(&self, workspace_path: &str) -> Result<PersistedReasoningClaim> {
        self.claim(workspace_path, CLAIM_REFACTOR_PLAN)
    }

    pub fn impact(&self, workspace_path: &str, args: &Value) -> Result<PersistedReasoningClaim> {
        let claim_id = impact_claim_id(args)?;
        self.parameterized_claim(workspace_path, &claim_id, |kernel, computed_at_us, data| {
            kernel.impact_claim(workspace_path, args, data, computed_at_us)
        })
    }

    pub fn history(&self, workspace_path: &str, args: &Value) -> Result<PersistedReasoningClaim> {
        let claim_id = history_claim_id(args);
        self.parameterized_claim(workspace_path, &claim_id, |kernel, computed_at_us, data| {
            kernel.history_claim(workspace_path, args, data, computed_at_us)
        })
    }

    pub fn search(
        &self,
        workspace_path: &str,
        query: &str,
        limit: usize,
    ) -> Result<PersistedReasoningClaim> {
        let claim_id = search_claim_id(query, limit);
        let subject = json!({
            "query": query,
            "limit": limit,
        });
        self.parameterized_claim(workspace_path, &claim_id, |kernel, computed_at_us, data| {
            kernel.search_claim(workspace_path, &subject, data, computed_at_us)
        })
    }

    pub fn safe_to_delete(
        &self,
        workspace_path: &str,
        context: &str,
        entity: &str,
    ) -> Result<PersistedReasoningClaim> {
        let claim_id = safe_to_delete_claim_id(context, entity);
        self.parameterized_claim(workspace_path, &claim_id, |kernel, computed_at_us, data| {
            kernel.safe_to_delete_claim(workspace_path, context, entity, data, computed_at_us)
        })
    }

    pub fn how_connected(
        &self,
        workspace_path: &str,
        from: &str,
        to: &str,
    ) -> Result<PersistedReasoningClaim> {
        self.how_connected_with_relation(workspace_path, "context_dep", from, to)
    }

    pub fn how_connected_with_relation(
        &self,
        workspace_path: &str,
        relation: &str,
        from: &str,
        to: &str,
    ) -> Result<PersistedReasoningClaim> {
        let relation = normalize_path_relation(relation)?;
        let claim_id = how_connected_claim_id_for_relation(relation, from, to);
        self.parameterized_claim(workspace_path, &claim_id, |kernel, computed_at_us, data| {
            kernel.how_connected_claim(workspace_path, relation, from, to, data, computed_at_us)
        })
    }

    pub fn eager_refresh_for_dependency(
        &self,
        workspace_path: &str,
        dependency_state: &str,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let stale_claims = self
            .store
            .load_stale_reasoning_claims_for_dependency(workspace_path, dependency_state)?;
        if stale_claims.is_empty() {
            return Ok(Vec::new());
        }
        self.eager_refresh_stale_claims_from_existing(workspace_path, &stale_claims)
    }

    pub fn eager_refresh_stale_claims(
        &self,
        workspace_path: &str,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let stale_claims = self.store.load_stale_reasoning_claims(workspace_path)?;
        if stale_claims.is_empty() {
            return Ok(Vec::new());
        }

        self.eager_refresh_stale_claims_from_existing(workspace_path, &stale_claims)
    }

    fn eager_refresh_stale_claims_from_existing(
        &self,
        workspace_path: &str,
        stale_claims: &[PersistedReasoningClaim],
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let data = self.load_materialized_data(workspace_path)?;
        let computed_at_us = now_us();
        let refreshed = stale_claims
            .iter()
            .map(|claim| self.rebuild_claim(workspace_path, claim, &data, computed_at_us))
            .collect::<Result<Vec<_>>>()?;
        self.store
            .upsert_reasoning_claims(workspace_path, &refreshed)?;
        Ok(refreshed)
    }

    pub fn refresh(&self, workspace_path: &str) -> Result<Vec<PersistedReasoningClaim>> {
        let claims = self.materialize_claims(workspace_path)?;
        self.store
            .upsert_reasoning_claims(workspace_path, &claims)?;
        Ok(claims)
    }

    fn claim(&self, workspace_path: &str, claim_id: &str) -> Result<PersistedReasoningClaim> {
        self.ensure_fresh(workspace_path, claim_id)?;
        self.store
            .load_reasoning_claim(workspace_path, claim_id)?
            .with_context(|| format!("reasoning claim '{claim_id}' was not materialized"))
    }

    fn ensure_fresh(&self, workspace_path: &str, claim_id: &str) -> Result<()> {
        self.store.reload_persisted_policy(workspace_path)?;
        let refresh_needed = match self.store.load_reasoning_claim(workspace_path, claim_id)? {
            Some(claim) => claim.stale,
            None => true,
        };

        if refresh_needed {
            self.refresh_claim_ids(workspace_path, &[claim_id.to_string()])?;
        }

        Ok(())
    }

    fn parameterized_claim<F>(
        &self,
        workspace_path: &str,
        claim_id: &str,
        builder: F,
    ) -> Result<PersistedReasoningClaim>
    where
        F: FnOnce(&Self, i64, &MaterializedReasoningData) -> Result<PersistedReasoningClaim>,
    {
        self.store.reload_persisted_policy(workspace_path)?;
        if let Some(claim) = self.store.load_reasoning_claim(workspace_path, claim_id)? {
            let basis = self.snapshot_basis(workspace_path)?;
            if !claim.stale && claim_dependencies_match_basis(&claim, &basis) {
                return Ok(claim);
            }
        }

        let data = self.load_materialized_data(workspace_path)?;
        let mut claim = builder(self, now_us(), &data)?;
        self.populate_default_justifications(&mut claim, &data);
        self.store
            .upsert_reasoning_claims(workspace_path, &[claim.clone()])?;
        Ok(claim)
    }

    fn refresh_claim_ids(
        &self,
        workspace_path: &str,
        claim_ids: &[String],
    ) -> Result<Vec<PersistedReasoningClaim>> {
        if claim_ids.is_empty() {
            return Ok(Vec::new());
        }

        let data = self.load_materialized_data(workspace_path)?;
        let computed_at_us = now_us();
        let claims = claim_ids
            .iter()
            .map(|claim_id| self.build_claim_by_id(workspace_path, claim_id, &data, computed_at_us))
            .collect::<Result<Vec<_>>>()?;
        self.store
            .upsert_reasoning_claims(workspace_path, &claims)?;
        Ok(claims)
    }

    fn materialize_claims(&self, workspace_path: &str) -> Result<Vec<PersistedReasoningClaim>> {
        let claim_ids = CANONICAL_CLAIM_IDS
            .iter()
            .map(|claim_id| (*claim_id).to_string())
            .collect::<Vec<_>>();
        let data = self.load_materialized_data(workspace_path)?;
        self.materialize_claims_for_ids(workspace_path, &claim_ids, &data, now_us())
    }

    fn load_materialized_data(&self, workspace_path: &str) -> Result<MaterializedReasoningData> {
        let canonical = canonicalize_path(workspace_path);
        let actual = self.store.load_actual(workspace_path)?;
        let truth = self.store.truth_maintenance_report(workspace_path)?;
        let basis = self.snapshot_basis(workspace_path)?;
        let layer_violations = self.store.layer_violations(&canonical)?;
        let circular_deps = self.store.circular_deps(&canonical)?;
        let aggregate_quality = self.store.aggregate_roots_without_invariants(&canonical)?;
        let health = self.store.model_health(&canonical)?;
        let policy_result = self.store.evaluate_policy_violations(&canonical)?;
        let pending_changes = self.store.diff_graph(workspace_path)?["pending_changes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let drift_entries = self
            .store
            .load_drift(workspace_path)?
            .into_iter()
            .map(|(category, context, name, change_type)| {
                let mut entry = json!({
                    "category": category,
                    "name": name,
                    "change_type": change_type,
                });
                if !context.is_empty() {
                    entry["context"] = json!(context);
                }
                entry
            })
            .collect();

        Ok(MaterializedReasoningData {
            basis,
            truth_assumptions: truth.assumptions,
            layer_violations,
            circular_deps,
            aggregate_quality,
            health,
            policy_result,
            pending_changes,
            drift_entries,
            has_actual: actual
                .as_ref()
                .is_some_and(|model| !model.bounded_contexts.is_empty()),
            ast_stats: build_ast_stats(actual.as_ref()),
        })
    }

    fn materialize_claims_for_ids(
        &self,
        workspace_path: &str,
        claim_ids: &[String],
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        claim_ids
            .iter()
            .map(|claim_id| self.build_claim_by_id(workspace_path, claim_id, data, computed_at_us))
            .collect()
    }

    fn build_claim_by_id(
        &self,
        workspace_path: &str,
        claim_id: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let mut claim = if let Some(rule) = canonical_claim_rule(claim_id) {
            (rule.build)(self, workspace_path, data, computed_at_us)?
        } else {
            let existing = self
                .store
                .load_reasoning_claim(workspace_path, claim_id)?
                .with_context(|| {
                    format!("reasoning claim '{claim_id}' is missing stored subject metadata")
                })?;
            self.build_parameterized_claim_from_existing(
                workspace_path,
                &existing,
                data,
                computed_at_us,
            )?
        };
        self.populate_default_justifications(&mut claim, data);
        Ok(claim)
    }

    fn rebuild_claim(
        &self,
        workspace_path: &str,
        claim: &PersistedReasoningClaim,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let mut rebuilt = if canonical_claim_rule(&claim.claim_id).is_some() {
            self.build_claim_by_id(workspace_path, &claim.claim_id, data, computed_at_us)?
        } else {
            self.build_parameterized_claim_from_existing(
                workspace_path,
                claim,
                data,
                computed_at_us,
            )?
        };
        self.populate_default_justifications(&mut rebuilt, data);
        Ok(rebuilt)
    }

    fn build_parameterized_claim_from_existing(
        &self,
        workspace_path: &str,
        claim: &PersistedReasoningClaim,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let rule = parameterized_claim_rule(&claim.claim_id)
            .with_context(|| format!("Unknown reasoning claim '{}'", claim.claim_id))?;
        (rule.build)(self, workspace_path, claim, data, computed_at_us)
    }

    fn populate_default_justifications(
        &self,
        claim: &mut PersistedReasoningClaim,
        data: &MaterializedReasoningData,
    ) {
        if claim.justifications.is_empty() {
            claim.justifications = default_justifications_for_claim(claim, data);
        }
    }

    fn snapshot_basis(&self, workspace_path: &str) -> Result<SnapshotBasis> {
        Ok(SnapshotBasis {
            desired_ts: self
                .store
                .list_snapshots(workspace_path, ACTUAL_STATE)?
                .into_iter()
                .next()
                .unwrap_or(0),
            actual_ts: self
                .store
                .list_snapshots(workspace_path, ACTUAL_STATE)?
                .into_iter()
                .next()
                .unwrap_or(0),
            drift_ts: self
                .store
                .load_drift_recomputed_at(workspace_path)?
                .unwrap_or(0),
        })
    }

    fn architecture_claim(
        &self,
        workspace_path: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let canonical = canonicalize_path(workspace_path);
        let mut implemented = build_model_overview_json(self.store, &canonical, "actual");
        let rust_ontology = build_rust_ontology_contract_json(self.store, &canonical, "actual");
        if !implemented.is_object() && rust_ontology["available"].as_bool().unwrap_or(false) {
            implemented = json!({});
        }
        if let Some(object) = implemented.as_object_mut() {
            object.insert(
                "ontology_contract".into(),
                json!({
                    "ground_truth": "rust",
                    "primary_nodes": ["workspace", "crate", "module", "submodule", "source_file", "symbol"],
                    "overview_nodes": ["crate", "module", "submodule", "struct"],
                    "semantic_overlays": ["entity_candidate", "value_object_candidate", "service_candidate", "repository_candidate", "event_candidate"],
                    "ui": "The web graph is an overview projection; the stored Rust fact graph remains complete for MCP reasoning."
                }),
            );
            object.insert("rust_ontology".into(), rust_ontology.clone());
        }
        let has_model = implemented
            .get("bounded_contexts")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
            || rust_ontology["available"].as_bool().unwrap_or(false);
        let pending_count = data.pending_changes.len();
        let status = if has_model {
            if pending_count == 0 {
                "ok"
            } else {
                "changed_since_previous_snapshot"
            }
        } else {
            "no_model"
        };

        Ok(build_claim(
            CLAIM_ARCHITECTURE_OVERVIEW,
            "architecture",
            "workspace",
            status,
            match status {
                "ok" => "Implemented Rust architecture graph is available.".into(),
                "changed_since_previous_snapshot" => format!(
                    "Rust architecture overview shows {pending_count} change(s) since the previous implemented snapshot."
                ),
                _ => "No implemented Rust architecture snapshot is stored.".into(),
            },
            json!({
                "implemented": if has_model { implemented.clone() } else { Value::Null },
                "current": if has_model { implemented } else { Value::Null },
                "status": status,
                "temporal_change_count": if has_model { pending_count } else { 0 },
                "health": health_json(&data.health),
            }),
            vec![ReasoningDerivation {
                rule: "architecture overview combines implemented graph reconstruction with health and temporal diff summary".into(),
                derived_from: vec![
                    "project".into(),
                    "source_file".into(),
                    "symbol".into(),
                    "import_edge".into(),
                    "calls_symbol".into(),
                    "context".into(),
                    "context_dep".into(),
                    "model_health".into(),
                    "diff_graph".into(),
                ],
                witness_count: pending_count,
            }],
            vec![support(
                "evidence",
                "Architecture overview inputs",
                json!({
                    "has_implemented_model": has_model,
                    "temporal_change_count": pending_count,
                    "health_score": data.health.score,
                }),
            )],
            data.truth_assumptions.clone(),
            vec![
                "Architecture overview is derived from persisted snapshots and does not include unstored editor changes.".into(),
            ],
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "architecture_overview",
            "actual_history",
        ))
    }

    fn history_claim(
        &self,
        workspace_path: &str,
        args: &Value,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let raw_state = args["state"].as_str().unwrap_or("actual");
        let state = canonical_model_state(raw_state);
        let has_timestamps = args["ts_old"].is_number() || args["ts_new"].is_number();
        let subject = json!({
            "state": raw_state,
            "ts_old": args["ts_old"],
            "ts_new": args["ts_new"],
        });

        let payload = if has_timestamps {
            let ts_old = args["ts_old"].as_i64().unwrap_or(0);
            let ts_new = args["ts_new"].as_i64().unwrap_or_else(now_us);
            let mut payload = self
                .store
                .diff_snapshots(workspace_path, state, ts_old, ts_new)?;
            payload["status"] = json!("comparison");
            payload
        } else {
            let timestamps = self.store.list_snapshots(workspace_path, state)?;
            json!({
                "status": "listing",
                "state": raw_state,
                "snapshots": timestamps,
                "count": timestamps.len(),
            })
        };

        Ok(build_claim(
            &history_claim_id(args),
            "history",
            &subject.to_string(),
            if has_timestamps { "comparison" } else { "listing" },
            if has_timestamps {
                format!("Compared {} snapshots for '{raw_state}' history.", payload["summary"]["total_changes"].as_u64().unwrap_or(0))
            } else {
                format!("Listed {} snapshot(s) for '{raw_state}' history.", payload["count"].as_u64().unwrap_or(0))
            },
            payload.clone(),
            vec![ReasoningDerivation {
                rule: if has_timestamps {
                    "snapshot history diff compares two stored temporal snapshots"
                } else {
                    "snapshot history listing returns recorded temporal snapshots for a state"
                }
                .into(),
                derived_from: vec!["snapshot_log".into()],
                witness_count: payload["count"].as_u64().unwrap_or_else(|| payload["summary"]["total_changes"].as_u64().unwrap_or(0)) as usize,
            }],
            vec![support("evidence", "History payload", payload)],
            data.truth_assumptions.clone(),
            vec![
                "History reflects persisted snapshots only; direct incremental mutations that do not record snapshots will not appear as separate history entries.".into(),
            ],
            vec![dependency("snapshot", state, data.basis.actual_ts)],
            computed_at_us,
            "snapshot_history",
            state,
        ))
    }

    fn search_claim(
        &self,
        workspace_path: &str,
        subject: &Value,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let query = subject["query"].as_str().unwrap_or("");
        let limit = subject["limit"].as_u64().unwrap_or(20) as usize;
        let payload = self.store.search_text(workspace_path, query, limit)?;
        let count = payload["count"].as_u64().unwrap_or(0) as usize;
        let includes_rust_fact_results = payload["results"].as_array().is_some_and(|results| {
            results.iter().any(|result| {
                result["search_mode"].as_str() == Some("rust_fact_scan")
                    || matches!(
                        result["kind"].as_str(),
                        Some("source_file" | "symbol" | "import_edge" | "calls_symbol")
                    )
            })
        });
        let includes_policy_results = payload["results"].as_array().is_some_and(|results| {
            results.iter().any(|result| {
                result["search_mode"].as_str() == Some("policy_scan")
                    || matches!(
                        result["kind"].as_str(),
                        Some("layer_assignment" | "dependency_constraint")
                    )
            })
        });
        let mut derived_from = vec![
            "context:fts".into(),
            "entity:fts".into(),
            "service:fts".into(),
            "event:fts".into(),
            "architectural_decision:title_fts".into(),
        ];
        if includes_rust_fact_results {
            derived_from.push("source_file".into());
            derived_from.push("symbol".into());
            derived_from.push("import_edge".into());
            derived_from.push("calls_symbol".into());
        }
        if includes_policy_results {
            derived_from.push("layer_assignment".into());
            derived_from.push("dependency_constraint".into());
        }
        let mut limitations = vec![
            "Search may omit unindexed relation fields; a model-scan fallback is used when FTS returns no rows."
                .into(),
        ];
        if includes_policy_results {
            limitations.push(
                "Policy search results come from exact stored policy relations, not Cozo FTS indices."
                    .into(),
            );
        }
        if includes_rust_fact_results {
            limitations.push(
                "Source-level Rust fact search uses stored scan relations when FTS has no row-level symbol index."
                    .into(),
            );
        }

        Ok(build_claim(
            &search_claim_id(query, limit),
            "search",
            &subject.to_string(),
            "results",
            format!("Search returned {count} architecture match(es) for '{query}'."),
            payload.clone(),
            vec![ReasoningDerivation {
                rule: "search matches persisted implemented architecture entities across indexed relations, policy relations, or model scan fallback".into(),
                derived_from,
                witness_count: count,
            }],
            vec![support("evidence", "Search results", payload)],
            vec![],
            limitations,
            vec![dependency("snapshot", "actual", data.basis.actual_ts)],
            computed_at_us,
            if includes_policy_results || includes_rust_fact_results {
                "architecture_search"
            } else {
                "fts_search"
            },
            "actual",
        ))
    }

    fn impact_claim(
        &self,
        workspace_path: &str,
        args: &Value,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let analysis = args["analysis"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("'analysis' parameter is required")?;
        let canonical = canonicalize_path(workspace_path);
        let subject = normalized_impact_subject(args);

        let (payload, derived_from, rule, limitations, dependencies, provenance_state) =
            match analysis {
                "transitive_deps" => {
                    let context = required_arg(args, "context", analysis)?;
                    let deps = self.store.transitive_deps(&canonical, &context)?;
                    (
                        json!({
                            "analysis": analysis,
                            "context": context,
                            "dependencies": deps,
                            "count": deps.len(),
                        }),
                        vec!["context_dep".into()],
                        "transitive dependency closure over the implemented context_dep graph"
                            .to_string(),
                        vec!["Only stored implemented context dependencies are traversed.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "circular_deps" => {
                    let cycles = self.store.circular_deps(&canonical)?;
                    let cycle_pairs: Vec<_> = cycles
                        .iter()
                        .map(|(from, to)| json!({ "from": from, "to": to }))
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "cycles": cycle_pairs,
                            "has_cycles": !cycles.is_empty(),
                            "count": cycles.len(),
                        }),
                        vec!["context_dep".into()],
                        "cycle detection over the implemented context_dep graph".to_string(),
                        vec!["Cycles are derived from declared context dependencies, not runtime communication paths.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "layer_violations" => {
                    let violations = self.store.layer_violations(&canonical)?;
                    let items: Vec<_> = violations
                        .iter()
                        .map(|(context, service, dependency)| {
                            json!({
                                "context": context,
                                "domain_service": service,
                                "infrastructure_dependency": dependency,
                            })
                        })
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "violations": items,
                            "count": violations.len(),
                        }),
                        vec!["service".into(), "service_dep".into()],
                        "layer violation analysis over implemented service/service_dep relations".to_string(),
                        vec!["Service layer classifications are taken from stored implemented declarations.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "impact_analysis" => {
                    let context = required_arg(args, "context", analysis)?;
                    let entity = required_arg(args, "entity", analysis)?;
                    let result = self.store.impact_analysis(&canonical, &context, &entity)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec![
                            "event".into(),
                            "repository".into(),
                            "service_dep".into(),
                            "context_dep".into(),
                            "ast_edge".into(),
                            "import_edge".into(),
                        ],
                        "implemented impact analysis for an entity within a bounded context".to_string(),
                        vec!["Impact analysis combines stored domain facts with scanned AST/import evidence.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "aggregate_quality" => {
                    let roots = self.store.aggregate_roots_without_invariants(&canonical)?;
                    let items: Vec<_> = roots
                        .iter()
                        .map(|(context, entity)| json!({ "context": context, "entity": entity }))
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "aggregate_roots_without_invariants": items,
                            "count": roots.len(),
                            "recommendation": if roots.is_empty() {
                                "All aggregate roots have invariants defined."
                            } else {
                                "Consider adding domain invariants to protect these aggregate roots."
                            },
                        }),
                        vec!["entity".into(), "invariant".into()],
                        "aggregate quality analysis over implemented entity and invariant relations".to_string(),
                        vec!["Aggregate quality analysis only considers stored implemented invariants.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "dependency_graph" => {
                    let graph = self.store.dependency_graph(&canonical)?;
                    (
                        json!({
                            "analysis": analysis,
                            "graph": graph,
                        }),
                        vec!["context".into(), "context_dep".into()],
                        "dependency graph projection over implemented contexts and dependencies"
                            .to_string(),
                        vec!["Dependency graph uses stored implemented boundaries only.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "field_usage" => {
                    let field_type = required_arg(args, "field_type", analysis)?;
                    let rows = self.store.field_usage(&canonical, &field_type)?;
                    let items: Vec<_> = rows
                        .iter()
                        .map(|(context, owner_kind, owner, field)| {
                            json!({
                                "context": context,
                                "owner_kind": owner_kind,
                                "owner": owner,
                                "field": field,
                            })
                        })
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "field_type": field_type,
                            "usages": items,
                            "count": rows.len(),
                        }),
                        vec!["field".into()],
                        "field usage query over implemented field relations".to_string(),
                        vec!["Field usage searches stored implemented field metadata only.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "method_search" => {
                    let method_name = required_arg(args, "method_name", analysis)?;
                    let rows = self.store.method_search(&canonical, &method_name)?;
                    let items: Vec<_> = rows
                        .iter()
                        .map(|(context, owner_kind, owner, return_type)| {
                            json!({
                                "context": context,
                                "owner_kind": owner_kind,
                                "owner": owner,
                                "return_type": return_type,
                            })
                        })
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "method_name": method_name,
                            "matches": items,
                            "count": rows.len(),
                        }),
                        vec!["method".into()],
                        "method search query over implemented method relations".to_string(),
                        vec!["Method search uses stored implemented method metadata only.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "shared_fields" => {
                    let rows = self.store.shared_fields(&canonical)?;
                    let items: Vec<_> = rows
                        .iter()
                        .map(|(context, entity, event, field, field_type)| {
                            json!({
                                "context": context,
                                "entity": entity,
                                "event": event,
                                "field": field,
                                "type": field_type,
                            })
                        })
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "shared": items,
                            "count": rows.len(),
                            "insight": if rows.is_empty() {
                                "No shared fields between entities and events."
                            } else {
                                "Shared fields suggest event-sourcing alignment. Events carry entity state."
                            },
                        }),
                        vec!["field".into()],
                        "shared-field analysis across implemented entity/event field relations".to_string(),
                        vec!["Shared-field analysis only considers stored implemented field metadata.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "pagerank" => {
                    match self.store.pagerank(&canonical) {
                        Ok(ranked) => {
                            let items: Vec<_> = ranked
                                .iter()
                                .map(|(context, rank)| json!({ "context": context, "rank": rank }))
                                .collect();
                            (
                                json!({
                                    "analysis": analysis,
                                    "available": true,
                                    "ranking": items,
                                    "count": ranked.len(),
                                    "insight": "Higher PageRank indicates more architecturally important contexts (more dependencies flow through them).",
                                }),
                                vec!["context_dep".into()],
                                "PageRank over implemented context dependency graph".to_string(),
                                vec!["PageRank is computed in-process (power iteration) over the context dependency graph.".into()],
                                vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                                "actual",
                            )
                        }
                        Err(error) => (
                            json!({
                                "analysis": analysis,
                                "available": false,
                                "ranking": [],
                                "count": 0,
                                "reason": error.to_string(),
                            }),
                            vec!["context_dep".into()],
                            "PageRank computation failed".to_string(),
                            vec!["PageRank failed to compute over the context dependency graph.".into()],
                            vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                            "actual",
                        ),
                    }
                }
                "community_detection" => {
                    match self.store.community_detection(&canonical) {
                        Ok(communities) => {
                            let items: Vec<_> = communities
                                .iter()
                                .map(|(context, community)| {
                                    json!({ "context": context, "community": community })
                                })
                                .collect();
                            (
                                json!({
                                    "analysis": analysis,
                                    "available": true,
                                    "communities": items,
                                    "count": communities.len(),
                                    "insight": "Contexts in the same community are tightly coupled. Consider aligning bounded context boundaries with community boundaries.",
                                }),
                                vec!["context_dep".into()],
                                "community detection over implemented context dependency graph".to_string(),
                                vec!["Community detection (label propagation) is computed in-process over the context dependency graph.".into()],
                                vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                                "actual",
                            )
                        }
                        Err(error) => (
                            json!({
                                "analysis": analysis,
                                "available": false,
                                "communities": [],
                                "count": 0,
                                "reason": error.to_string(),
                            }),
                            vec!["context_dep".into()],
                            "community detection computation failed".to_string(),
                            vec!["Community detection failed to compute over the context dependency graph.".into()],
                            vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                            "actual",
                        ),
                    }
                }
                "betweenness_centrality" => {
                    match self.store.betweenness_centrality(&canonical) {
                        Ok(ranked) => {
                            let items: Vec<_> = ranked
                                .iter()
                                .map(|(context, centrality)| {
                                    json!({ "context": context, "centrality": centrality })
                                })
                                .collect();
                            (
                                json!({
                                    "analysis": analysis,
                                    "available": true,
                                    "ranking": items,
                                    "count": ranked.len(),
                                    "insight": "High betweenness centrality indicates bottleneck contexts. Changes here have outsized downstream impact.",
                                }),
                                vec!["context_dep".into()],
                                "betweenness centrality over implemented context dependency graph".to_string(),
                                vec!["Betweenness centrality (Brandes) is computed in-process over the context dependency graph.".into()],
                                vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                                "actual",
                            )
                        }
                        Err(error) => (
                            json!({
                                "analysis": analysis,
                                "available": false,
                                "ranking": [],
                                "count": 0,
                                "reason": error.to_string(),
                            }),
                            vec!["context_dep".into()],
                            "betweenness centrality computation failed".to_string(),
                            vec!["Betweenness centrality failed to compute over the context dependency graph.".into()],
                            vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                            "actual",
                        ),
                    }
                }
                "degree_centrality" => {
                    let degrees = self.store.degree_centrality(&canonical)?;
                    let items: Vec<_> = degrees
                        .iter()
                        .map(|(context, in_degree, out_degree)| {
                            json!({
                                "context": context,
                                "in_degree": in_degree,
                                "out_degree": out_degree,
                            })
                        })
                        .collect();
                    (
                        json!({
                            "analysis": analysis,
                            "degrees": items,
                            "count": degrees.len(),
                        }),
                        vec!["context".into(), "context_dep".into()],
                        "degree centrality over implemented context dependency graph".to_string(),
                        vec!["Degree centrality is computed from stored implemented context dependencies only.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "topological_order" => {
                    let result = self.store.topological_order(&canonical)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec!["context".into(), "context_dep".into()],
                        "topological ordering over implemented context dependency graph".to_string(),
                        vec!["Topological ordering is only possible when the stored context dependency graph is acyclic.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "call_graph_callers" => {
                    let symbol = required_arg(args, "symbol", analysis)?;
                    let result = self.store.call_graph_callers(&canonical, &symbol)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec!["calls_symbol".into()],
                        "direct caller lookup over actual-state call graph".to_string(),
                        vec![
                            "Call graph queries depend on the latest successful actual-state scan."
                                .into(),
                        ],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "call_graph_callees" => {
                    let symbol = required_arg(args, "symbol", analysis)?;
                    let result = self.store.call_graph_callees(&canonical, &symbol)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec!["calls_symbol".into()],
                        "direct callee lookup over actual-state call graph".to_string(),
                        vec![
                            "Call graph queries depend on the latest successful actual-state scan."
                                .into(),
                        ],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "call_graph_reachability" => {
                    let symbol = required_arg(args, "symbol", analysis)?;
                    let result = self.store.call_graph_reachability(&canonical, &symbol)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec!["calls_symbol".into()],
                        "transitive reachability over actual-state call graph".to_string(),
                        vec!["Reachability is limited to static call edges extracted during the latest actual-state scan.".into()],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "call_graph_stats" => {
                    let result = self.store.call_graph_stats(&canonical)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec!["calls_symbol".into(), "symbol".into()],
                        "call graph summary statistics over actual-state call edges and project symbol aliases".to_string(),
                        vec![
                            "Call graph statistics depend on the latest successful actual-state scan.".into(),
                            "Project callee statistics match extracted callee names to stored symbol aliases; ambiguous short names may match multiple project symbols.".into(),
                        ],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "optimization_recommendations" => {
                    let result = self.store.optimization_recommendations(&canonical)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec![
                            "source_file".into(),
                            "symbol".into(),
                            "import_edge".into(),
                            "calls_symbol".into(),
                            "ast_edge".into(),
                        ],
                        "graph-derived refactoring and optimization recommendation heuristics over actual-state Rust facts".to_string(),
                        vec![
                            "Recommendations are static candidates, not automatic edits or proven runtime wins.".into(),
                            "Use rust_resolve, cargo test, benchmarks, and rust_diff to validate any proposed shape.".into(),
                        ],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                "practice_findings" => {
                    let result = self.store.practice_findings(&canonical)?;
                    (
                        json!({
                            "analysis": analysis,
                            "result": result,
                        }),
                        vec![
                            "symbol".into(),
                            "calls_symbol".into(),
                            "ast_edge".into(),
                            "reference_edge".into(),
                        ],
                        "graph-ranked Rust practice findings over actual-state source facts".to_string(),
                        vec![
                            "Practice findings are static triage signals, not proof of a bug.".into(),
                            "Use cargo check, Clippy, tests, and local review to validate remediation.".into(),
                        ],
                        vec![dependency("snapshot", "actual", data.basis.actual_ts)],
                        "actual",
                    )
                }
                other => anyhow::bail!(
                    "Unknown analysis type: '{}'. Valid types: transitive_deps, circular_deps, layer_violations, impact_analysis, aggregate_quality, dependency_graph, field_usage, method_search, shared_fields, pagerank, community_detection, betweenness_centrality, degree_centrality, topological_order, call_graph_callers, call_graph_callees, call_graph_reachability, call_graph_stats, optimization_recommendations, practice_findings",
                    other
                ),
            };

        let witness_count = impact_witness_count(&payload);

        Ok(build_claim(
            &impact_claim_id(args)?,
            "impact",
            &subject.to_string(),
            "ok",
            format!("Impact analysis '{analysis}' completed."),
            payload.clone(),
            vec![ReasoningDerivation {
                rule,
                derived_from,
                witness_count,
            }],
            vec![support("evidence", "Impact analysis result", payload)],
            data.truth_assumptions.clone(),
            limitations,
            dependencies,
            computed_at_us,
            "impact_analysis",
            provenance_state,
        ))
    }

    fn check_layer_violations_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let items: Vec<Value> = data
            .layer_violations
            .iter()
            .map(|(context, service, dependency)| {
                json!({
                    "context": context,
                    "domain_service": service,
                    "infrastructure_dependency": dependency,
                })
            })
            .collect();
        build_claim(
            CLAIM_CHECK_LAYER_VIOLATIONS,
            "check",
            "layer_violations",
            if data.layer_violations.is_empty() { "true" } else { "false" },
            if data.layer_violations.is_empty() {
                "No layer violations detected.".to_string()
            } else {
                format!("{} layer violation(s) found.", data.layer_violations.len())
            },
            json!({
                "invariant": "layer_violations",
                "status": if data.layer_violations.is_empty() { "true" } else { "false" },
                "violations": items.clone(),
                "count": data.layer_violations.len(),
            }),
            vec![ReasoningDerivation {
                rule: "domain_service MUST_NOT depend_on infrastructure_service".into(),
                derived_from: vec!["service".into(), "service_dep".into()],
                witness_count: data.layer_violations.len(),
            }],
            vec![support(
                "evidence",
                "Layer violation witnesses",
                json!(items),
            )],
            vec![],
            vec![
                "Results are limited to stored implemented service classifications and dependencies.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn check_circular_deps_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let pairs: Vec<Value> = data
            .circular_deps
            .iter()
            .map(|(from, to)| json!({ "from": from, "to": to }))
            .collect();
        build_claim(
            CLAIM_CHECK_CIRCULAR_DEPS,
            "check",
            "circular_deps",
            if data.circular_deps.is_empty() { "true" } else { "false" },
            if data.circular_deps.is_empty() {
                "No circular dependencies detected.".into()
            } else {
                format!("{} circular dependency pair(s) found.", data.circular_deps.len())
            },
            json!({
                "invariant": "circular_deps",
                "status": if data.circular_deps.is_empty() { "true" } else { "false" },
                "cycles": pairs.clone(),
                "count": data.circular_deps.len(),
            }),
            vec![ReasoningDerivation {
                rule: "context_dep graph MUST be acyclic".into(),
                derived_from: vec!["context_dep".into()],
                witness_count: data.circular_deps.len(),
            }],
            vec![support("evidence", "Circular dependency witnesses", json!(pairs))],
            vec![],
            vec![
                "Cycles are computed from declared context dependencies, not runtime communication paths.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn check_aggregate_quality_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let items: Vec<Value> = data
            .aggregate_quality
            .iter()
            .map(|(context, entity)| json!({ "context": context, "entity": entity }))
            .collect();
        build_claim(
            CLAIM_CHECK_AGGREGATE_QUALITY,
            "check",
            "aggregate_quality",
            if data.aggregate_quality.is_empty() { "true" } else { "false" },
            if data.aggregate_quality.is_empty() {
                "All aggregate roots have invariants.".into()
            } else {
                format!(
                    "{} aggregate root(s) without invariants found.",
                    data.aggregate_quality.len()
                )
            },
            json!({
                "invariant": "aggregate_quality",
                "status": if data.aggregate_quality.is_empty() { "true" } else { "false" },
                "roots_without_invariants": items.clone(),
                "count": data.aggregate_quality.len(),
            }),
            vec![ReasoningDerivation {
                rule: "aggregate_root MUST have at_least_one invariant".into(),
                derived_from: vec!["entity".into(), "invariant".into()],
                witness_count: data.aggregate_quality.len(),
            }],
            vec![support(
                "evidence",
                "Aggregate roots missing invariants",
                json!(items),
            )],
            vec![],
            vec![
                "Aggregate quality is evaluated from stored implemented entity and invariant relations.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn check_orphan_contexts_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let orphans = data.health.orphan_contexts.clone();
        build_claim(
            CLAIM_CHECK_ORPHAN_CONTEXTS,
            "check",
            "orphan_contexts",
            if orphans.is_empty() { "true" } else { "false" },
            if orphans.is_empty() {
                "No orphan contexts detected.".into()
            } else {
                format!("{} orphan context(s) found.", orphans.len())
            },
            json!({
                "invariant": "orphan_contexts",
                "status": if orphans.is_empty() { "true" } else { "false" },
                "orphans": orphans.clone(),
                "count": orphans.len(),
            }),
            vec![ReasoningDerivation {
                rule: "context SHOULD participate_in dependency_graph".into(),
                derived_from: vec!["context".into(), "context_dep".into()],
                witness_count: orphans.len(),
            }],
            vec![support("evidence", "Orphan contexts", json!(orphans))],
            vec![],
            vec![
                "Orphan detection is derived from stored implemented contexts and dependencies."
                    .into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn check_policy_violations_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let violations = data.policy_result["violations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let count = data.policy_result["count"].as_u64().unwrap_or(0) as usize;
        let configured = data.policy_result["configured"].as_bool().unwrap_or(false);
        let policy_coverage = data.policy_result["policy_coverage"].clone();
        let mut limitations = vec![
            "Policy evaluation depends on declared constraints and stored implemented dependencies."
                .into(),
        ];
        if !configured {
            limitations.push(
                "Policy coverage is incomplete; an empty violation set is not proof of compliance."
                    .into(),
            );
        }
        build_claim(
            CLAIM_CHECK_POLICY_VIOLATIONS,
            "check",
            "policy_violations",
            data.policy_result["status"].as_str().unwrap_or("unknown"),
            if !configured {
                "Policy constraints are not fully configured.".into()
            } else if violations.is_empty() {
                "No policy violations detected.".into()
            } else {
                format!("{} policy violation(s) found.", violations.len())
            },
            json!({
                "invariant": "policy_violations",
                "status": data.policy_result["status"],
                "configured": configured,
                "policy_coverage": policy_coverage,
                "violations": data.policy_result["violations"],
                "count": data.policy_result["count"],
            }),
            vec![ReasoningDerivation {
                rule: "dependency MUST_NOT violate declared constraint".into(),
                derived_from: vec![
                    "context_dep".into(),
                    "layer_assignment".into(),
                    "dependency_constraint".into(),
                ],
                witness_count: count,
            }],
            vec![support(
                "evidence",
                "Policy evaluation evidence",
                json!({
                    "configured": configured,
                    "policy_coverage": data.policy_result["policy_coverage"],
                    "violations": violations,
                }),
            )],
            vec![],
            limitations,
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn check_drift_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        build_claim(
            CLAIM_CHECK_DRIFT,
            "check",
            "drift",
            if data.pending_changes.is_empty() { "true" } else { "false" },
            if data.pending_changes.is_empty() {
                "No implemented graph changes detected between recent snapshots.".into()
            } else {
                format!("{} implemented graph change(s) detected.", data.pending_changes.len())
            },
            json!({
                "check_name": "drift",
                "status": if data.pending_changes.is_empty() { "true" } else { "false" },
                "pending_changes": data.pending_changes.len(),
            }),
            vec![ReasoningDerivation {
                rule: "implemented graph has no temporal drift IFF diff_graph has no pending changes"
                    .into(),
                derived_from: vec![
                    "context".into(),
                    "entity".into(),
                    "service".into(),
                    "event".into(),
                    "value_object".into(),
                    "repository".into(),
                    "module".into(),
                ],
                witness_count: data.pending_changes.len(),
            }],
            vec![support(
                "evidence",
                "Implemented graph changes",
                json!(data.pending_changes),
            )],
            data.truth_assumptions.clone(),
            vec![
                "Drift freshness depends on whether compute_drift has been rerun after the latest implemented graph snapshot.".into(),
            ],
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual_history",
        )
    }

    fn check_all_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let policy_count = data.policy_result["count"].as_u64().unwrap_or(0);
        let policy_configured = data.policy_result["configured"].as_bool().unwrap_or(false);
        let results = json!({
            "layer_violations": {
                "status": data.layer_violations.is_empty(),
                "count": data.layer_violations.len(),
            },
            "circular_deps": {
                "status": data.circular_deps.is_empty(),
                "count": data.circular_deps.len(),
            },
            "aggregate_quality": {
                "status": data.aggregate_quality.is_empty(),
                "count": data.aggregate_quality.len(),
            },
            "orphan_contexts": {
                "status": data.health.orphan_contexts.is_empty(),
                "count": data.health.orphan_contexts.len(),
            },
            "policy_violations": {
                "status": policy_count == 0,
                "count": policy_count,
            },
            "policy_coverage": {
                "status": policy_configured,
                "configured": policy_configured,
                "coverage": data.policy_result["policy_coverage"],
            },
            "drift": {
                "status": data.pending_changes.is_empty(),
                "count": data.pending_changes.len(),
            }
        });
        let all_pass = data.layer_violations.is_empty()
            && data.circular_deps.is_empty()
            && data.aggregate_quality.is_empty()
            && data.health.orphan_contexts.is_empty()
            && policy_count == 0
            && policy_configured
            && data.pending_changes.is_empty();
        build_claim(
            CLAIM_CHECK_ALL,
            "check",
            "all",
            if all_pass { "pass" } else { "issues_found" },
            if all_pass {
                "All curated architectural checks passed.".into()
            } else {
                "One or more curated architectural checks failed.".into()
            },
            json!({
                "check_name": "all",
                "status": if all_pass { "pass" } else { "issues_found" },
                "checks": results.clone(),
            }),
            vec![ReasoningDerivation {
                rule: "all curated architectural invariants must hold simultaneously".into(),
                derived_from: vec![
                    "layer_violations".into(),
                    "circular_deps".into(),
                    "aggregate_quality".into(),
                    "orphan_contexts".into(),
                    "policy_violations".into(),
                    "drift".into(),
                ],
                witness_count: if all_pass { 0 } else { 1 },
            }],
            vec![support(
                "evidence",
                "Aggregate check results",
                results.clone(),
            )],
            data.truth_assumptions.clone(),
            vec![
                "This aggregate result combines implemented invariants with temporal drift status."
                    .into(),
            ],
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual_history",
        )
    }

    fn why_layer_violations_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let evidence: Vec<Value> = data
            .layer_violations
            .iter()
            .map(|(context, service, dependency)| {
                json!({
                    "context": context,
                    "domain_service": service,
                    "infrastructure_dependency": dependency,
                    "explanation": format!(
                        "Service '{service}' in context '{context}' depends on '{dependency}', which is an infrastructure-layer dependency. Domain services must not depend on infrastructure directly."
                    ),
                    "rule": "domain_service MUST NOT depend_on infrastructure_dependency",
                })
            })
            .collect();
        let no_violations = evidence.is_empty();
        build_claim(
            CLAIM_WHY_LAYER_VIOLATIONS,
            "explanation",
            "layer_violations",
            if no_violations { "true" } else { "false" },
            if no_violations {
                "No layer violations detected. All domain services depend only on domain-level abstractions.".into()
            } else {
                format!(
                    "{} layer violation(s) found. Domain services reference infrastructure dependencies directly, violating the dependency inversion principle.",
                    evidence.len()
                )
            },
            json!({
                "violation_type": "layer_violations",
                "status": if no_violations { "true" } else { "false" },
                "explanation": if no_violations {
                    "No layer violations detected. All domain services depend only on domain-level abstractions.".to_string()
                } else {
                    format!(
                        "{} layer violation(s) found. Domain services reference infrastructure dependencies directly, violating the dependency inversion principle.",
                        evidence.len()
                    )
                },
                "remediation": if no_violations {
                    Value::Null
                } else {
                    json!("Introduce abstractions (traits/interfaces) in the domain layer and inject infrastructure implementations.")
                },
            }),
            vec![ReasoningDerivation {
                rule: "domain_service MUST NOT depend_on infrastructure_dependency".into(),
                derived_from: vec!["service".into(), "service_dep".into()],
                witness_count: evidence.len(),
            }],
            vec![support("evidence", "Layer violation explanations", json!(evidence))],
            vec![],
            vec![
                "Explanations are synthesized from stored witnesses and do not inspect implementation code paths directly.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn why_circular_deps_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let evidence: Vec<Value> = data
            .circular_deps
            .iter()
            .map(|(from, to)| {
                json!({
                    "from": from,
                    "to": to,
                    "explanation": format!(
                        "Context '{from}' depends on '{to}' and '{to}' depends on '{from}', forming a circular dependency cycle."
                    ),
                    "rule": "context dependency graph MUST be acyclic",
                })
            })
            .collect();
        let no_cycles = evidence.is_empty();
        build_claim(
            CLAIM_WHY_CIRCULAR_DEPS,
            "explanation",
            "circular_deps",
            if no_cycles { "true" } else { "false" },
            if no_cycles {
                "No circular dependencies detected. Context dependency graph is acyclic.".into()
            } else {
                format!(
                    "{} circular dependency pair(s) found. Cycles prevent clean module boundaries.",
                    evidence.len()
                )
            },
            json!({
                "violation_type": "circular_deps",
                "status": if no_cycles { "true" } else { "false" },
                "explanation": if no_cycles {
                    "No circular dependencies detected. Context dependency graph is acyclic.".to_string()
                } else {
                    format!(
                        "{} circular dependency pair(s) found. Cycles prevent clean module boundaries.",
                        evidence.len()
                    )
                },
                "remediation": if no_cycles {
                    Value::Null
                } else {
                    json!("Break cycles by extracting shared concepts into a new context or using events for decoupling.")
                },
            }),
            vec![ReasoningDerivation {
                rule: "context dependency graph MUST be acyclic".into(),
                derived_from: vec!["context_dep".into()],
                witness_count: evidence.len(),
            }],
            vec![support(
                "evidence",
                "Circular dependency explanations",
                json!(evidence),
            )],
            vec![],
            vec![
                "Cycle explanations are synthesized from stored witnesses, not runtime traces."
                    .into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn why_policy_violations_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let violations = data.policy_result["violations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let evidence: Vec<Value> = violations
            .iter()
            .map(|violation| {
                let kind = violation["kind"].as_str().unwrap_or("?");
                json!({
                    "kind": kind,
                    "from_context": violation["from_context"],
                    "to_context": violation["to_context"],
                    "from_layer": violation["from_layer"],
                    "to_layer": violation["to_layer"],
                    "rule": violation["rule"],
                    "explanation": if kind == "layer" {
                        format!(
                            "Context '{}' (layer: {}) depends on '{}' (layer: {}), violating forbidden layer dependency.",
                            violation["from_context"].as_str().unwrap_or("?"),
                            violation["from_layer"].as_str().unwrap_or("?"),
                            violation["to_context"].as_str().unwrap_or("?"),
                            violation["to_layer"].as_str().unwrap_or("?"),
                        )
                    } else {
                        format!(
                            "Context '{}' depends on '{}', violating forbidden context dependency.",
                            violation["from_context"].as_str().unwrap_or("?"),
                            violation["to_context"].as_str().unwrap_or("?"),
                        )
                    }
                })
            })
            .collect();
        let clean = evidence.is_empty();
        build_claim(
            CLAIM_WHY_POLICY_VIOLATIONS,
            "explanation",
            "policy_violations",
            if clean { "true" } else { "false" },
            if clean {
                "No policy violations detected. All dependencies conform to declared constraints.".into()
            } else {
                format!(
                    "{} policy violation(s) found. Dependencies violate declared architectural constraints.",
                    evidence.len()
                )
            },
            json!({
                "violation_type": "policy_violations",
                "status": if clean { "true" } else { "false" },
                "explanation": if clean {
                    "No policy violations detected. All dependencies conform to declared constraints.".to_string()
                } else {
                    format!(
                        "{} policy violation(s) found. Dependencies violate declared architectural constraints.",
                        evidence.len()
                    )
                },
                "remediation": if clean {
                    Value::Null
                } else {
                    json!("Review forbidden dependencies and refactor to respect layer boundaries.")
                },
            }),
            vec![ReasoningDerivation {
                rule: "dependency MUST_NOT violate declared constraint".into(),
                derived_from: vec![
                    "context_dep".into(),
                    "layer_assignment".into(),
                    "dependency_constraint".into(),
                ],
                witness_count: evidence.len(),
            }],
            vec![support("evidence", "Policy violation explanations", json!(evidence))],
            vec![],
            vec![
                "Policy explanations depend on declared constraints and stored implemented dependencies.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn why_aggregate_quality_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let evidence: Vec<Value> = data
            .aggregate_quality
            .iter()
            .map(|(context, entity)| {
                json!({
                    "context": context,
                    "entity": entity,
                    "explanation": format!(
                        "Entity '{entity}' in context '{context}' is marked as aggregate root but has no invariants defined. Aggregate roots should enforce domain invariants."
                    ),
                    "rule": "aggregate_root MUST have at_least_one invariant",
                })
            })
            .collect();
        let clean = evidence.is_empty();
        build_claim(
            CLAIM_WHY_AGGREGATE_QUALITY,
            "explanation",
            "aggregate_quality",
            if clean { "true" } else { "false" },
            if clean {
                "All aggregate root entities have at least one invariant defined.".into()
            } else {
                format!(
                    "{} aggregate root(s) without invariants. Domain integrity may be at risk.",
                    evidence.len()
                )
            },
            json!({
                "violation_type": "aggregate_quality",
                "status": if clean { "true" } else { "false" },
                "explanation": if clean {
                    "All aggregate root entities have at least one invariant defined.".to_string()
                } else {
                    format!(
                        "{} aggregate root(s) without invariants. Domain integrity may be at risk.",
                        evidence.len()
                    )
                },
                "remediation": if clean {
                    Value::Null
                } else {
                    json!("Add invariants to aggregate roots to express domain rules explicitly.")
                },
            }),
            vec![ReasoningDerivation {
                rule: "aggregate_root MUST have at_least_one invariant".into(),
                derived_from: vec!["entity".into(), "invariant".into()],
                witness_count: evidence.len(),
            }],
            vec![support(
                "evidence",
                "Aggregate quality explanations",
                json!(evidence),
            )],
            vec![],
            vec![
                "Aggregate-quality explanations are synthesized from stored witnesses, not runtime aggregate behavior.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn why_orphan_contexts_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let evidence: Vec<Value> = data
            .health
            .orphan_contexts
            .iter()
            .map(|context| {
                json!({
                    "context": context,
                    "explanation": format!(
                        "Context '{context}' has no dependencies to or from other contexts. It may be unused or missing declared relationships."
                    ),
                    "rule": "context SHOULD participate_in dependency_graph",
                })
            })
            .collect();
        let clean = evidence.is_empty();
        build_claim(
            CLAIM_WHY_ORPHAN_CONTEXTS,
            "explanation",
            "orphan_contexts",
            if clean { "true" } else { "false" },
            if clean {
                "No orphan contexts. All contexts participate in the dependency graph.".into()
            } else {
                format!(
                    "{} orphan context(s) found. These contexts are isolated from the dependency graph.",
                    evidence.len()
                )
            },
            json!({
                "violation_type": "orphan_contexts",
                "status": if clean { "true" } else { "false" },
                "explanation": if clean {
                    "No orphan contexts. All contexts participate in the dependency graph.".to_string()
                } else {
                    format!(
                        "{} orphan context(s) found. These contexts are isolated from the dependency graph.",
                        evidence.len()
                    )
                },
                "remediation": if clean {
                    Value::Null
                } else {
                    json!("Add dependencies or remove unused contexts.")
                },
            }),
            vec![ReasoningDerivation {
                rule: "context SHOULD participate_in dependency_graph".into(),
                derived_from: vec!["context".into(), "context_dep".into()],
                witness_count: evidence.len(),
            }],
            vec![support(
                "evidence",
                "Orphan context explanations",
                json!(evidence),
            )],
            vec![],
            vec![
                "Orphan-context explanations are derived from stored implemented boundaries, not runtime interaction data.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual",
        )
    }

    fn drift_overview_claim(
        &self,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let added: Vec<Value> = data
            .pending_changes
            .iter()
            .filter(|change| change["action"].as_str() == Some("add"))
            .cloned()
            .collect();
        let removed: Vec<Value> = data
            .pending_changes
            .iter()
            .filter(|change| change["action"].as_str() == Some("remove"))
            .cloned()
            .collect();
        build_claim(
            CLAIM_DRIFT_OVERVIEW,
            "drift",
            "actual_history",
            if data.pending_changes.is_empty() {
                "in_sync"
            } else {
                "pending_changes"
            },
            if data.pending_changes.is_empty() {
                "No implemented graph changes detected between the two latest snapshots.".into()
            } else {
                format!(
                    "{} implemented graph change(s) detected between the two latest snapshots.",
                    data.pending_changes.len()
                )
            },
            json!({
                "status": if data.pending_changes.is_empty() { "in_sync" } else { "pending_changes" },
                "summary": {
                    "total_changes": data.pending_changes.len(),
                    "additions": added.len(),
                    "removals": removed.len(),
                    "drift_entries": data.drift_entries.len(),
                },
                "added": added,
                "removed": removed,
                "drift": data.drift_entries.clone(),
            }),
            vec![ReasoningDerivation {
                rule: "persisted drift is the temporal set difference between recent implemented graph snapshots".into(),
                derived_from: vec!["diff_graph".into(), "drift".into()],
                witness_count: data.pending_changes.len(),
            }],
            vec![support(
                "evidence",
                "Pending changes and persisted drift",
                json!({
                    "pending_changes": data.pending_changes,
                    "persisted_drift": data.drift_entries,
                }),
            )],
            data.truth_assumptions.clone(),
            vec![
                "Persisted drift entries can be stale if the implemented graph changed after the last drift recomputation.".into(),
            ],
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "datalog",
            "actual_history",
        )
    }

    fn diagnose_claim(
        &self,
        workspace_path: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> PersistedReasoningClaim {
        let policy_count = data.policy_result["count"].as_u64().unwrap_or(0);
        let practice_findings = self
            .store
            .practice_findings(workspace_path)
            .unwrap_or_else(|_| json!({ "findings": [], "count": 0 }));
        let top_practice_findings = practice_findings["findings"]
            .as_array()
            .map(|findings| findings.iter().take(3).cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let invariants = json!({
            "circular_deps": {
                "status": if data.circular_deps.is_empty() { "pass" } else { "fail" },
                "count": data.circular_deps.len(),
                "cycles": data.circular_deps.iter().map(|(from, to)| json!({"from": from, "to": to})).collect::<Vec<_>>(),
            },
            "layer_violations": {
                "status": if data.layer_violations.is_empty() { "pass" } else { "fail" },
                "count": data.layer_violations.len(),
                "violations": data.layer_violations.iter().map(|(context, service, dependency)| json!({
                    "context": context,
                    "service": service,
                    "dependency": dependency,
                })).collect::<Vec<_>>(),
            },
            "aggregate_quality": {
                "status": if data.aggregate_quality.is_empty() { "pass" } else { "fail" },
                "count": data.aggregate_quality.len(),
                "roots_without_invariants": data.aggregate_quality.iter().map(|(context, entity)| json!({
                    "context": context,
                    "entity": entity,
                })).collect::<Vec<_>>(),
            },
            "policy_violations": {
                "status": data.policy_result["status"],
                "count": data.policy_result["count"],
                "violations": data.policy_result["violations"],
            }
        });

        let mut next_actions = Vec::new();
        let mut priority = 0u32;
        if !data.has_actual {
            next_actions.push(json!({
                "priority": priority,
                "tool": "sync",
                "reason": "No implemented model exists. Scan the workspace to extract architecture from source code.",
            }));
            priority += 1;
        }
        if !data.circular_deps.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "define",
                "reason": format!(
                    "{} circular dependency cycle(s) detected. Break cycles by extracting shared concepts or using events.",
                    data.circular_deps.len()
                ),
                "evidence": data.circular_deps.iter().map(|(from, to)| format!("{from} ⇄ {to}")).collect::<Vec<_>>(),
            }));
            priority += 1;
        }
        if !data.layer_violations.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "define",
                "reason": format!(
                    "{} layer violation(s). Domain services depend on infrastructure directly. Invert via ports/adapters.",
                    data.layer_violations.len()
                ),
                "evidence": data.layer_violations.iter().map(|(context, service, dependency)| format!("{context}.{service} → {dependency}")).collect::<Vec<_>>(),
            }));
            priority += 1;
        }
        if policy_count > 0 {
            next_actions.push(json!({
                "priority": priority,
                "tool": "constrain",
                "action": "evaluate",
                "reason": format!("{policy_count} policy violation(s). Declared constraints are not met."),
            }));
            priority += 1;
        }
        if !data.aggregate_quality.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "define",
                "reason": format!(
                    "{} aggregate root(s) without invariants. Add business rules to protect consistency.",
                    data.aggregate_quality.len()
                ),
                "evidence": data.aggregate_quality.iter().map(|(context, entity)| format!("{context}.{entity}")).collect::<Vec<_>>(),
            }));
            priority += 1;
        }
        if !data.health.unsourced_events.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "define",
                "reason": format!(
                    "{} event(s) without a source entity. Link them to their originating aggregate.",
                    data.health.unsourced_events.len()
                ),
                "evidence": data.health.unsourced_events.iter().map(|[context, event]| format!("{context}.{event}")).collect::<Vec<_>>(),
            }));
            priority += 1;
        }
        if !data.health.orphan_contexts.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "define",
                "reason": format!(
                    "{} orphan context(s) with no dependencies. Add dependencies or verify they are intentionally standalone.",
                    data.health.orphan_contexts.len()
                ),
                "evidence": data.health.orphan_contexts,
            }));
        }
        if !data
            .health
            .policy_coverage
            .missing_layer_assignments
            .is_empty()
            || data.health.policy_coverage.dependency_constraint_count == 0
        {
            let mut evidence = Vec::new();
            if !data
                .health
                .policy_coverage
                .missing_layer_assignments
                .is_empty()
            {
                evidence.push(json!({
                    "missing_layer_assignments": data.health.policy_coverage.missing_layer_assignments,
                }));
            }
            if data.health.policy_coverage.dependency_constraint_count == 0 {
                evidence.push(json!({
                    "missing_dependency_constraints": true,
                }));
            }
            next_actions.push(json!({
                "priority": priority,
                "tool": "constrain",
                "action": "list",
                "reason": "Architecture policy coverage is incomplete; health is not fully constrained.",
                "evidence": evidence,
            }));
        }
        if !data.pending_changes.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "refactor",
                "action": "plan",
                "reason": format!(
                    "{} implemented graph change(s) detected between recent snapshots. Run 'plan' for details.",
                    data.pending_changes.len()
                ),
            }));
        }
        if !top_practice_findings.is_empty() {
            next_actions.push(json!({
                "priority": priority,
                "tool": "rust_impact",
                "action": "practice_findings",
                "reason": format!(
                    "{} Rust practice finding(s) ranked by graph evidence. Inspect top findings before broad refactors.",
                    practice_findings["count"].as_u64().unwrap_or(top_practice_findings.len() as u64)
                ),
                "evidence": top_practice_findings.clone(),
            }));
        }
        if next_actions.is_empty() && data.has_actual {
            next_actions.push(json!({
                "priority": 0,
                "tool": "sync",
                "reason": "Architecture is healthy (score 100). Re-scan periodically to verify after code changes.",
            }));
        }

        let mut failing_invariants = Vec::new();
        if !data.circular_deps.is_empty() {
            failing_invariants.push("circular_deps");
        }
        if !data.layer_violations.is_empty() {
            failing_invariants.push("layer_violations");
        }
        if !data.aggregate_quality.is_empty() {
            failing_invariants.push("aggregate_quality");
        }
        if policy_count > 0 {
            failing_invariants.push("policy_violations");
        }

        build_claim(
            CLAIM_DIAGNOSE_REFACTOR,
            "diagnose",
            "refactor",
            if data.health.score == 100 {
                "healthy"
            } else if data.health.score >= 70 {
                "needs_improvement"
            } else {
                "unhealthy"
            },
            format!("Architecture health score: {}.", data.health.score),
            json!({
                "status": if data.health.score == 100 { "healthy" } else if data.health.score >= 70 { "needs_improvement" } else { "unhealthy" },
                "health_score": data.health.score,
                "health": health_json(&data.health),
                "invariants": invariants,
                "drift": {
                    "status": if data.pending_changes.is_empty() { "in_sync" } else { "drifted" },
                    "pending_change_count": data.pending_changes.len(),
                    "pending_changes": data.pending_changes,
                },
                "ast_edges": data.ast_stats,
                "practice_findings": {
                    "count": practice_findings["count"],
                    "top": top_practice_findings,
                },
                "has_implemented_model": data.has_actual,
                "next_actions": next_actions,
                "loop_hint": "After implementing fixes, call sync then diagnose again to verify improvement.",
            }),
            vec![ReasoningDerivation {
                rule: "diagnose composes persisted health, invariant, drift, AST, and Rust practice analyses into a prioritized refactoring report".into(),
                derived_from: vec![
                    "model_health".into(),
                    "circular_deps".into(),
                    "layer_violations".into(),
                    "aggregate_roots_without_invariants".into(),
                    "evaluate_policy_violations".into(),
                    "diff_graph".into(),
                    "load_actual".into(),
                    "practice_findings".into(),
                ],
                witness_count: failing_invariants.len() + 1,
            }],
            vec![support(
                "evidence",
                "Diagnose summary inputs",
                json!({
                    "has_implemented_model": data.has_actual,
                    "failing_invariants": failing_invariants,
                    "next_action_count": next_actions.len(),
                }),
            )],
            data.truth_assumptions.clone(),
            diagnose_limitations(data),
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "analysis_pipeline",
            "actual_history",
        )
    }

    fn refactor_plan_claim_from_data(
        &self,
        workspace_path: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        if data.pending_changes.is_empty() {
            return Ok(build_claim(
                CLAIM_REFACTOR_PLAN,
                "refactor_plan",
                "refactor",
                "in_sync",
                "No implemented graph changes detected between recent snapshots.".into(),
                json!({
                    "status": "in_sync",
                    "message": "No implemented graph changes detected between recent snapshots.",
                }),
                vec![ReasoningDerivation {
                    rule: "refactor plan is empty IFF the recent implemented graph diff has no pending changes".into(),
                    derived_from: vec!["diff_graph".into()],
                    witness_count: 0,
                }],
                vec![support(
                    "evidence",
                    "No pending refactor changes",
                    json!({
                        "pending_changes": [],
                        "change_count": 0,
                    }),
                )],
                data.truth_assumptions.clone(),
                vec![
                    "Refactor planning operates on persisted implemented graph snapshots, not direct workspace diffs.".into(),
                    "A fresh sync is required after code changes to keep the implemented model current.".into(),
                ],
                cross_state_dependencies(&data.basis),
                computed_at_us,
                "refactor_lifecycle",
                "actual_history",
            ));
        }

        let enriched = enrich_plan(self.store, workspace_path, &data.pending_changes);
        Ok(build_claim(
            CLAIM_REFACTOR_PLAN,
            "refactor_plan",
            "refactor",
            "pending_changes",
            format!("{} implemented graph change(s) need review.", data.pending_changes.len()),
            enriched,
            vec![ReasoningDerivation {
                rule: "refactor plan is derived from recent implemented graph diff ordered by structural priority heuristics".into(),
                derived_from: vec![
                    "diff_graph".into(),
                    "model_health".into(),
                    "kind_priority".into(),
                    "suggest_file".into(),
                ],
                witness_count: data.pending_changes.len(),
            }],
            vec![support(
                "evidence",
                "Raw refactor plan inputs",
                json!({
                    "raw_pending_changes": data.pending_changes,
                }),
            )],
            data.truth_assumptions.clone(),
            vec![
                "Suggested files and execution priority are heuristics derived from module paths and change kinds.".into(),
                "The plan reflects persisted implemented graph snapshots rather than unstored editor changes.".into(),
            ],
            cross_state_dependencies(&data.basis),
            computed_at_us,
            "refactor_lifecycle",
            "actual_history",
        ))
    }

    fn safe_to_delete_claim(
        &self,
        workspace_path: &str,
        context: &str,
        entity: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let canonical = canonicalize_path(workspace_path);
        let result = self.store.can_delete_symbol(&canonical, context, entity)?;
        let can_delete = result["can_delete"].as_bool().unwrap_or(false);

        let mut assumptions = Vec::new();
        if !data.has_actual {
            assumptions.push(
                "No implemented domain model is stored; aggregate-, event-, and repository-level witnesses may be incomplete."
                    .to_string(),
            );
        }
        if !data.has_actual {
            assumptions.push(
                "No actual model is stored; import-, AST-, and call-graph references are based on empty scan data."
                    .to_string(),
            );
        }

        Ok(build_claim(
            &safe_to_delete_claim_id(context, entity),
            "safe_to_delete",
            &json!({ "context": context, "entity": entity }).to_string(),
            if can_delete { "true" } else { "false" },
            if can_delete {
                format!("'{entity}' in context '{context}' has no inbound references in the stored graph.")
            } else {
                format!("'{entity}' in context '{context}' still has inbound references in the stored graph.")
            },
            json!({
                "status": if can_delete { "true" } else { "false" },
                "context": context,
                "entity": entity,
                "can_delete": can_delete,
                "result": result.clone(),
            }),
            vec![ReasoningDerivation {
                rule: "entity deletable IFF no inbound references are present in the stored implemented graph".into(),
                derived_from: vec![
                    "aggregate_member".into(),
                    "event".into(),
                    "repository".into(),
                    "field".into(),
                    "method".into(),
                    "method_param".into(),
                    "import_edge".into(),
                    "ast_edge".into(),
                    "calls_symbol".into(),
                ],
                witness_count: witness_count_from_value(&result),
            }],
            vec![support(
                "evidence",
                "Inbound reference witnesses",
                json!({
                    "inbound_references": result,
                }),
            )],
            assumptions,
            vec![
                "Dynamic dispatch, reflection, string-based lookups, and out-of-repository consumers are not tracked.".into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "deletion_safety",
            "actual",
        ))
    }

    fn how_connected_claim(
        &self,
        workspace_path: &str,
        relation: &str,
        from: &str,
        to: &str,
        data: &MaterializedReasoningData,
        computed_at_us: i64,
    ) -> Result<PersistedReasoningClaim> {
        let canonical = canonicalize_path(workspace_path);
        if relation == "calls_symbol" {
            let paths = self.store.query_call_paths(&canonical, from, to)?;
            let result = self.store.call_graph_reachability(&canonical, from)?;
            let reachable_symbols = result["reachable"].as_array().cloned().unwrap_or_default();
            let reachable = !paths.is_empty();
            let assumptions = if !data.has_actual {
                vec![
                    "No implemented domain model is stored; connectivity is derived from an empty call graph."
                        .to_string(),
                ]
            } else {
                Vec::new()
            };
            return Ok(build_claim(
                &how_connected_claim_id_for_relation(relation, from, to),
                "how_connected",
                &json!({ "relation": relation, "from": from, "to": to }).to_string(),
                if reachable {
                    "reachable"
                } else {
                    "disconnected"
                },
                if reachable {
                    format!("'{from}' reaches '{to}' through stored call graph edges.")
                } else {
                    format!("No stored call graph path from '{from}' to '{to}'.")
                },
                json!({
                    "status": if reachable { "reachable" } else { "disconnected" },
                    "relation": relation,
                    "from": from,
                    "to": to,
                    "reachable": reachable,
                    "paths": paths.clone(),
                    "path_count": paths.len(),
                    "reachable_symbols": reachable_symbols,
                    "reachable_count": result["count"].clone(),
                }),
                vec![ReasoningDerivation {
                    rule: "transitive reachability via calls_symbol".into(),
                    derived_from: vec!["calls_symbol".into()],
                    witness_count: paths.len(),
                }],
                vec![support(
                    "evidence",
                    "Call graph reachability witnesses",
                    json!({ "paths": paths, "result": result }),
                )],
                assumptions,
                vec![
                    "Call graph reachability is computed from stored implemented calls_symbol edges and does not reconstruct every intermediate call path."
                        .into(),
                ],
                actual_dependencies(&data.basis),
                computed_at_us,
                "call_graph_reachability",
                "actual",
            ));
        }

        let paths = self.store.query_dependency_path(&canonical, from, to)?;
        let reachable = !paths.is_empty();
        let assumptions = if !data.has_actual {
            vec![
                "No implemented domain model is stored; connectivity is derived from an empty dependency graph."
                    .to_string(),
            ]
        } else {
            Vec::new()
        };

        Ok(build_claim(
            &how_connected_claim_id_for_relation(relation, from, to),
            "how_connected",
            &json!({ "relation": relation, "from": from, "to": to }).to_string(),
            if reachable {
                "reachable"
            } else {
                "disconnected"
            },
            if reachable {
                format!("'{from}' reaches '{to}' through stored context dependencies.")
            } else {
                format!("No stored dependency path from '{from}' to '{to}'.")
            },
            json!({
                "status": if reachable { "reachable" } else { "disconnected" },
                "relation": relation,
                "from": from,
                "to": to,
                "paths": paths.clone(),
                "reachable": reachable,
                "hop_count": paths.len(),
            }),
            vec![ReasoningDerivation {
                rule: "transitive reachability via context_dep".into(),
                derived_from: vec!["context_dep".into()],
                witness_count: paths.len(),
            }],
            vec![support(
                "evidence",
                "Dependency path witnesses",
                json!({ "paths": paths }),
            )],
            assumptions,
            vec![
                "Connectivity is computed from stored implemented context dependencies only."
                    .into(),
            ],
            actual_dependencies(&data.basis),
            computed_at_us,
            "dependency_path",
            "actual",
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_claim(
    claim_id: &str,
    claim_kind: &str,
    subject: &str,
    status: &str,
    summary: String,
    payload: Value,
    derivations: Vec<ReasoningDerivation>,
    supports: Vec<ReasoningSupportEdge>,
    assumptions: Vec<String>,
    limitations: Vec<String>,
    dependencies: Vec<ReasoningDependency>,
    computed_at_us: i64,
    provenance_source: &str,
    provenance_state: &str,
) -> PersistedReasoningClaim {
    let mut claim_assumptions: Vec<ReasoningAssumption> = assumptions
        .into_iter()
        .map(|text| ReasoningAssumption {
            assumption_kind: "assumption".into(),
            text,
        })
        .collect();
    claim_assumptions.extend(limitations.into_iter().map(|text| ReasoningAssumption {
        assumption_kind: "limitation".into(),
        text,
    }));

    PersistedReasoningClaim {
        claim_id: claim_id.into(),
        claim_kind: claim_kind.into(),
        subject: subject.into(),
        status: status.into(),
        summary,
        payload,
        provenance: ReasoningProvenance {
            source: provenance_source.into(),
            state: provenance_state.into(),
        },
        stale: false,
        computed_at_us,
        derivations,
        assumptions: claim_assumptions,
        supports,
        dependencies,
        justifications: Vec::new(),
    }
}

fn support(support_kind: &str, summary: &str, detail: Value) -> ReasoningSupportEdge {
    ReasoningSupportEdge {
        support_kind: support_kind.into(),
        summary: summary.into(),
        detail,
    }
}

fn dependency(
    dependency_kind: &str,
    dependency_state: &str,
    basis_timestamp_us: i64,
) -> ReasoningDependency {
    ReasoningDependency {
        dependency_kind: dependency_kind.into(),
        dependency_state: dependency_state.into(),
        basis_timestamp_us,
    }
}

fn claim_dependencies_match_basis(claim: &PersistedReasoningClaim, basis: &SnapshotBasis) -> bool {
    if claim.dependencies.is_empty() {
        return false;
    }
    claim.dependencies.iter().all(|dependency| {
        match (
            dependency.dependency_kind.as_str(),
            dependency.dependency_state.as_str(),
        ) {
            ("snapshot", "actual") => dependency.basis_timestamp_us == basis.actual_ts,
            ("materialized", "drift") => dependency.basis_timestamp_us == basis.drift_ts,
            _ => true,
        }
    })
}

fn actual_dependencies(basis: &SnapshotBasis) -> Vec<ReasoningDependency> {
    vec![dependency("snapshot", "actual", basis.actual_ts)]
}

fn cross_state_dependencies(basis: &SnapshotBasis) -> Vec<ReasoningDependency> {
    vec![
        dependency("snapshot", "actual", basis.actual_ts),
        dependency("materialized", "drift", basis.drift_ts),
    ]
}

fn safe_to_delete_claim_id(context: &str, entity: &str) -> String {
    format!("{CLAIM_SAFE_TO_DELETE_PREFIX}:{context}:{entity}")
}

#[cfg(test)]
fn how_connected_claim_id(from: &str, to: &str) -> String {
    how_connected_claim_id_for_relation("context_dep", from, to)
}

fn how_connected_claim_id_for_relation(relation: &str, from: &str, to: &str) -> String {
    if relation == "context_dep" {
        format!("{CLAIM_HOW_CONNECTED_PREFIX}:{from}:{to}")
    } else {
        format!("{CLAIM_HOW_CONNECTED_PREFIX}:{relation}:{from}:{to}")
    }
}

fn normalize_path_relation(relation: &str) -> Result<&'static str> {
    match relation {
        "" | "all" | "context_dep" => Ok("context_dep"),
        "calls_symbol" => Ok("calls_symbol"),
        other => anyhow::bail!("Unknown path relation '{other}'. Use context_dep or calls_symbol."),
    }
}

fn parse_json_subject(subject: &str) -> Value {
    serde_json::from_str(subject).unwrap_or_else(|_| json!({}))
}

fn witness_count_from_value(result: &Value) -> usize {
    let direct = [
        "aggregates_referencing",
        "events_sourced",
        "repositories_managing",
        "import_references",
        "ast_references",
        "call_references",
    ]
    .into_iter()
    .map(|field| {
        result[field]
            .as_array()
            .map(|items| items.len())
            .unwrap_or(0)
    })
    .sum::<usize>();
    let type_refs = result["type_references"]
        .as_object()
        .map(|references| {
            references
                .values()
                .map(|value| value.as_array().map(|items| items.len()).unwrap_or(0))
                .sum::<usize>()
        })
        .unwrap_or(0);
    direct + type_refs
}

fn build_ast_stats(actual: Option<&crate::domain::model::DomainModel>) -> Option<Value> {
    actual.map(|model| {
        let mut by_type: HashMap<&str, usize> = HashMap::new();
        for edge in &model.ast_edges {
            *by_type.entry(edge.edge_type.as_str()).or_default() += 1;
        }
        let breakdown: serde_json::Map<String, Value> = by_type
            .into_iter()
            .map(|(kind, count)| (kind.to_string(), json!(count)))
            .collect();
        json!({
            "total": model.ast_edges.len(),
            "by_type": breakdown,
        })
    })
}

fn health_json(health: &ModelHealth) -> Value {
    json!({
        "score": health.score,
        "circular_deps": health.circular_deps,
        "module_cycles": health.module_cycles,
        "layer_violations": health.layer_violations.iter().map(|violation| json!({
            "context": violation.context,
            "domain_service": violation.domain_service,
            "infra_dependency": violation.infra_dependency,
        })).collect::<Vec<_>>(),
        "missing_invariants": health.missing_invariants,
        "orphan_contexts": health.orphan_contexts,
        "god_contexts": health.god_contexts,
        "unsourced_events": health.unsourced_events,
        "policy_coverage": health.policy_coverage,
    })
}

fn diagnose_limitations(data: &MaterializedReasoningData) -> Vec<String> {
    let mut limitations = vec![
        "Diagnose relies on persisted snapshots and static structural analysis rather than runtime behavior.".into(),
        "Health and invariant results are only as fresh as the last successful sync.".into(),
    ];
    if !data.has_actual {
        limitations.push(
            "No implemented model is available; recommendations are based on empty scan data."
                .into(),
        );
    }
    limitations
}

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

fn enrich_plan(store: &Store, workspace_path: &str, changes: &[Value]) -> Value {
    let module_paths: HashMap<String, String> = store
        .run_datalog(
            "?[name, module_path] := *context{workspace: $ws, name, module_path, state: 'desired' @ 'NOW'}",
            workspace_path,
        )
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row[0].clone(), row[1].clone()))
        .collect();

    let mut enriched: Vec<Value> = changes
        .iter()
        .map(|change| {
            let kind = change["kind"].as_str().unwrap_or("");
            let action = change["action"].as_str().unwrap_or("");
            let name = change["name"].as_str().unwrap_or("");
            let context = change["context"].as_str().unwrap_or("");

            let module_path = if kind == "context" {
                module_paths.get(name).cloned().unwrap_or_default()
            } else {
                module_paths.get(context).cloned().unwrap_or_default()
            };

            let suggested_file = suggest_file(&module_path, kind, name);
            let priority = kind_priority(kind);

            let rationale = match (action, kind) {
                ("add", "context") => format!("Create bounded context '{name}' module structure"),
                ("remove", "context") => format!("Remove bounded context '{name}' and its module"),
                ("add", "entity") => format!("Add entity '{name}' to context '{context}'"),
                ("remove", "entity") => {
                    format!("Remove entity '{name}' from context '{context}'")
                }
                ("add", "service") => format!("Implement service '{name}' in context '{context}'"),
                ("remove", "service") => {
                    format!("Remove service '{name}' from context '{context}'")
                }
                ("add", other) => format!("Add {other} '{name}' in context '{context}'"),
                ("remove", other) => {
                    format!("Remove {other} '{name}' from context '{context}'")
                }
                _ => String::new(),
            };

            let mut entry = change.clone();
            entry["priority"] = json!(priority);
            entry["suggested_file"] = json!(suggested_file);
            entry["rationale"] = json!(rationale);
            entry
        })
        .collect();

    enriched.sort_by_key(|entry| entry["priority"].as_u64().unwrap_or(99));

    let health_score = store
        .model_health(workspace_path)
        .map(|health| health.score)
        .unwrap_or(0);

    json!({
        "status": "pending_changes",
        "pending_changes": enriched,
        "change_count": changes.len(),
        "health_score": health_score,
        "migration_notes": [
            "Apply changes in priority order (0 = highest).",
            "Context-level changes should be done before entity/service changes.",
            "Run `rust_scan` after implementing to update the actual Rust graph.",
            "In actual-first mode, `accept` and `reset` are compatibility no-ops."
        ]
    })
}

fn kind_priority(kind: &str) -> u8 {
    match kind {
        "context" => 0,
        "entity" => 1,
        "service" => 2,
        "repository" => 3,
        "value_object" => 4,
        "event" => 5,
        "invariant" | "field" | "method" => 6,
        _ => 7,
    }
}

fn suggest_file(module_path: &str, kind: &str, name: &str) -> String {
    if module_path.is_empty() {
        return String::new();
    }
    let snake = crate::domain::to_snake(name);
    match kind {
        "context" => format!("{module_path}/mod.rs"),
        "entity" | "value_object" => format!("{module_path}/model.rs"),
        "service" | "repository" => format!("{module_path}/{snake}.rs"),
        "event" => format!("{module_path}/events.rs"),
        "field" | "method" | "invariant" => format!("{module_path}/mod.rs"),
        _ => String::new(),
    }
}

const CANONICAL_CLAIM_RULES: [CanonicalClaimRule; 16] = [
    CanonicalClaimRule {
        id: CLAIM_ARCHITECTURE_OVERVIEW,
        build: build_architecture_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_LAYER_VIOLATIONS,
        build: build_check_layer_violations_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_CIRCULAR_DEPS,
        build: build_check_circular_deps_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_AGGREGATE_QUALITY,
        build: build_check_aggregate_quality_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_ORPHAN_CONTEXTS,
        build: build_check_orphan_contexts_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_POLICY_VIOLATIONS,
        build: build_check_policy_violations_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_DRIFT,
        build: build_check_drift_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_CHECK_ALL,
        build: build_check_all_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_WHY_LAYER_VIOLATIONS,
        build: build_why_layer_violations_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_WHY_CIRCULAR_DEPS,
        build: build_why_circular_deps_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_WHY_POLICY_VIOLATIONS,
        build: build_why_policy_violations_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_WHY_AGGREGATE_QUALITY,
        build: build_why_aggregate_quality_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_WHY_ORPHAN_CONTEXTS,
        build: build_why_orphan_contexts_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_DRIFT_OVERVIEW,
        build: build_drift_overview_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_DIAGNOSE_REFACTOR,
        build: build_diagnose_refactor_rule,
    },
    CanonicalClaimRule {
        id: CLAIM_REFACTOR_PLAN,
        build: build_refactor_plan_rule,
    },
];

const PARAMETERIZED_CLAIM_RULES: [ParameterizedClaimRule; 5] = [
    ParameterizedClaimRule {
        prefix: CLAIM_SAFE_TO_DELETE_PREFIX,
        build: rebuild_safe_to_delete_rule,
    },
    ParameterizedClaimRule {
        prefix: CLAIM_HOW_CONNECTED_PREFIX,
        build: rebuild_how_connected_rule,
    },
    ParameterizedClaimRule {
        prefix: CLAIM_IMPACT_PREFIX,
        build: rebuild_impact_rule,
    },
    ParameterizedClaimRule {
        prefix: CLAIM_HISTORY_PREFIX,
        build: rebuild_history_rule,
    },
    ParameterizedClaimRule {
        prefix: CLAIM_SEARCH_PREFIX,
        build: rebuild_search_rule,
    },
];

fn canonical_claim_rule(claim_id: &str) -> Option<CanonicalClaimRule> {
    CANONICAL_CLAIM_RULES
        .iter()
        .copied()
        .find(|rule| rule.id == claim_id)
}

fn parameterized_claim_rule(claim_id: &str) -> Option<ParameterizedClaimRule> {
    PARAMETERIZED_CLAIM_RULES
        .iter()
        .copied()
        .find(|rule| claim_id == rule.prefix || claim_id.starts_with(&format!("{}:", rule.prefix)))
}

fn build_architecture_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    kernel.architecture_claim(workspace_path, data, computed_at_us)
}

fn build_check_layer_violations_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_layer_violations_claim(data, computed_at_us))
}

fn build_check_circular_deps_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_circular_deps_claim(data, computed_at_us))
}

fn build_check_aggregate_quality_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_aggregate_quality_claim(data, computed_at_us))
}

fn build_check_orphan_contexts_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_orphan_contexts_claim(data, computed_at_us))
}

fn build_check_policy_violations_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_policy_violations_claim(data, computed_at_us))
}

fn build_check_drift_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_drift_claim(data, computed_at_us))
}

fn build_check_all_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.check_all_claim(data, computed_at_us))
}

fn build_why_layer_violations_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.why_layer_violations_claim(data, computed_at_us))
}

fn build_why_circular_deps_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.why_circular_deps_claim(data, computed_at_us))
}

fn build_why_policy_violations_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.why_policy_violations_claim(data, computed_at_us))
}

fn build_why_aggregate_quality_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.why_aggregate_quality_claim(data, computed_at_us))
}

fn build_why_orphan_contexts_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.why_orphan_contexts_claim(data, computed_at_us))
}

fn build_drift_overview_rule(
    kernel: &ReasoningKernel<'_>,
    _workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.drift_overview_claim(data, computed_at_us))
}

fn build_diagnose_refactor_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    Ok(kernel.diagnose_claim(workspace_path, data, computed_at_us))
}

fn build_refactor_plan_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    kernel.refactor_plan_claim_from_data(workspace_path, data, computed_at_us)
}

fn rebuild_safe_to_delete_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    let subject = parse_json_subject(&claim.subject);
    let context = subject["context"].as_str().unwrap_or("");
    let entity = subject["entity"].as_str().unwrap_or("");
    if context.is_empty() || entity.is_empty() {
        anyhow::bail!(
            "reasoning claim '{}' is missing safe_to_delete subject metadata",
            claim.claim_id
        );
    }
    kernel.safe_to_delete_claim(workspace_path, context, entity, data, computed_at_us)
}

fn rebuild_how_connected_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    let subject = parse_json_subject(&claim.subject);
    let from = subject["from"].as_str().unwrap_or("");
    let to = subject["to"].as_str().unwrap_or("");
    if from.is_empty() || to.is_empty() {
        anyhow::bail!(
            "reasoning claim '{}' is missing how_connected subject metadata",
            claim.claim_id
        );
    }
    let relation = subject["relation"].as_str().unwrap_or("context_dep");
    let relation = normalize_path_relation(relation)?;
    kernel.how_connected_claim(workspace_path, relation, from, to, data, computed_at_us)
}

fn rebuild_impact_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    let subject = parse_json_subject(&claim.subject);
    kernel.impact_claim(workspace_path, &subject, data, computed_at_us)
}

fn rebuild_history_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    let subject = parse_json_subject(&claim.subject);
    kernel.history_claim(workspace_path, &subject, data, computed_at_us)
}

fn rebuild_search_rule(
    kernel: &ReasoningKernel<'_>,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
    computed_at_us: i64,
) -> Result<PersistedReasoningClaim> {
    let subject = parse_json_subject(&claim.subject);
    kernel.search_claim(workspace_path, &subject, data, computed_at_us)
}

fn impact_claim_id(args: &Value) -> Result<String> {
    let subject = normalized_impact_subject(args);
    let analysis = subject["analysis"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("'analysis' parameter is required")?;
    let mut parts = vec![
        CLAIM_IMPACT_PREFIX.to_string(),
        escape_claim_component(analysis),
    ];
    for key in ["context", "entity", "symbol", "field_type", "method_name"] {
        if let Some(value) = subject[key].as_str().filter(|value| !value.is_empty()) {
            parts.push(format!("{key}={}", escape_claim_component(value)));
        }
    }
    Ok(parts.join(":"))
}

fn history_claim_id(args: &Value) -> String {
    let raw_state = args["state"].as_str().unwrap_or("actual");
    match (args["ts_old"].as_i64(), args["ts_new"].as_i64()) {
        (Some(ts_old), ts_new) => format!(
            "{CLAIM_HISTORY_PREFIX}:{}:{}:{}",
            escape_claim_component(raw_state),
            ts_old,
            ts_new.unwrap_or(0)
        ),
        _ => format!(
            "{CLAIM_HISTORY_PREFIX}:{}",
            escape_claim_component(raw_state)
        ),
    }
}

fn search_claim_id(query: &str, limit: usize) -> String {
    format!(
        "{CLAIM_SEARCH_PREFIX}:{}:{}",
        escape_claim_component(query),
        limit
    )
}

fn normalized_impact_subject(args: &Value) -> Value {
    json!({
        "analysis": args["analysis"].as_str().unwrap_or(""),
        "context": args["context"].as_str().or_else(|| args["module"].as_str()).unwrap_or(""),
        "entity": args["entity"].as_str().or_else(|| args["struct"].as_str()).unwrap_or(""),
        "symbol": args["symbol"].as_str().or_else(|| args["struct"].as_str()).unwrap_or(""),
        "field_type": args["field_type"].as_str().unwrap_or(""),
        "method_name": args["method_name"].as_str().unwrap_or(""),
    })
}

fn required_arg(args: &Value, key: &str, analysis: &str) -> Result<String> {
    let aliases: Vec<&str> = match key {
        "context" => vec!["context", "module"],
        "entity" => vec!["entity", "struct"],
        "symbol" => vec!["symbol", "struct"],
        _ => vec![key],
    };
    aliases
        .iter()
        .find_map(|alias| args[*alias].as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| {
            format!(
                "'{}' parameter is required for {}",
                aliases.join("' or '"),
                analysis
            )
        })
}

fn escape_claim_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn impact_witness_count(payload: &Value) -> usize {
    payload["count"]
        .as_u64()
        .or_else(|| payload["result"]["count"].as_u64())
        .or_else(|| payload["result"]["total_edges"].as_u64())
        .or_else(|| payload["total_edges"].as_u64())
        .unwrap_or(0) as usize
}

fn build_model_overview_json(store: &Store, workspace: &str, state: &str) -> Value {
    let project = store
        .run_datalog(
            "?[name, description, tech_stack_json, conventions_json, rules_json] := \
                *project{workspace: $ws, name, description, tech_stack_json, conventions_json, rules_json}",
            workspace,
        )
        .unwrap_or_default();

    let contexts = store
        .run_datalog(
            &format!(
                "?[name, description, module_path] := \
                    *context{{workspace: $ws, name, description, module_path, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let (proj_name, proj_desc, tech, conventions, rules) = if let Some(row) = project.first() {
        (
            row[0].clone(),
            row[1].clone(),
            serde_json::from_str::<Value>(&row[2]).unwrap_or(json!({})),
            serde_json::from_str::<Value>(&row[3]).unwrap_or(json!({})),
            serde_json::from_str::<Value>(&row[4]).unwrap_or(json!([])),
        )
    } else if contexts.is_empty() {
        return json!({});
    } else {
        let fallback_name = std::path::Path::new(workspace)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unnamed".into());
        (
            fallback_name,
            String::new(),
            json!({}),
            json!({}),
            json!([]),
        )
    };

    let context_deps = store
        .run_datalog(
            &format!(
                "?[from_ctx, to_ctx] := \
                    *context_dep{{workspace: $ws, from_ctx, to_ctx, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let entities = store
        .run_datalog(
            &format!(
                "?[ctx, name, description, aggregate_root] := \
                    *entity{{workspace: $ws, context: ctx, name, description, aggregate_root, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let services = store
        .run_datalog(
            &format!(
                "?[ctx, name, description, kind] := \
                    *service{{workspace: $ws, context: ctx, name, description, kind, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let events = store
        .run_datalog(
            &format!(
                "?[ctx, name, description, source] := \
                    *event{{workspace: $ws, context: ctx, name, description, source, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let value_objects = store
        .run_datalog(
            &format!(
                "?[ctx, name, description] := \
                    *value_object{{workspace: $ws, context: ctx, name, description, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let repositories = store
        .run_datalog(
            &format!(
                "?[ctx, name, aggregate] := \
                    *repository{{workspace: $ws, context: ctx, name, aggregate, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let fields = store
        .run_datalog(
            &format!(
                "?[ctx, owner_kind, owner, name, field_type, required] := \
                    *field{{workspace: $ws, context: ctx, owner_kind, owner, name, field_type, required, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let methods = store
        .run_datalog(
            &format!(
                "?[ctx, owner_kind, owner, name, description, return_type] := \
                    *method{{workspace: $ws, context: ctx, owner_kind, owner, name, description, return_type, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let method_params = store
        .run_datalog(
            &format!(
                "?[ctx, owner_kind, owner, method, name, param_type, required] := \
                    *method_param{{workspace: $ws, context: ctx, owner_kind, owner, method, name, param_type, required, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let invariants = store
        .run_datalog(
            &format!(
                "?[ctx, entity, text] := \
                    *invariant{{workspace: $ws, context: ctx, entity, text, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let vo_rules = store
        .run_datalog(
            &format!(
                "?[ctx, vo, text] := \
                    *vo_rule{{workspace: $ws, context: ctx, value_object: vo, text, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let bounded_contexts: Vec<Value> = contexts
        .iter()
        .map(|ctx_row| {
            let ctx_name = &ctx_row[0];

            let depends_on: Vec<&str> = context_deps
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|row| row[1].as_str())
                .collect();

            let ctx_entities: Vec<Value> = entities
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|entity| {
                    let entity_name = &entity[1];
                    let entity_fields: Vec<Value> = fields
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name && row[1] == "entity" && row[2] == *entity_name
                        })
                        .map(|row| {
                            json!({
                                "name": row[3],
                                "type": row[4],
                                "required": row[5] == "true",
                            })
                        })
                        .collect();
                    let entity_methods: Vec<Value> = methods
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name && row[1] == "entity" && row[2] == *entity_name
                        })
                        .map(|row| {
                            let params: Vec<Value> = method_params
                                .iter()
                                .filter(|param| {
                                    param[0] == *ctx_name
                                        && param[1] == "entity"
                                        && param[2] == *entity_name
                                        && param[3] == row[3]
                                })
                                .map(|param| {
                                    json!({
                                        "name": param[4],
                                        "type": param[5],
                                        "required": param[6] == "true",
                                    })
                                })
                                .collect();
                            json!({
                                "name": row[3],
                                "description": row[4],
                                "return_type": row[5],
                                "parameters": params,
                            })
                        })
                        .collect();
                    let entity_invariants: Vec<&str> = invariants
                        .iter()
                        .filter(|row| row[0] == *ctx_name && row[1] == *entity_name)
                        .map(|row| row[2].as_str())
                        .collect();
                    json!({
                        "name": entity_name,
                        "description": entity[2],
                        "aggregate_root": entity[3] == "true",
                        "fields": entity_fields,
                        "methods": entity_methods,
                        "invariants": entity_invariants,
                    })
                })
                .collect();

            let ctx_services: Vec<Value> = services
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|service| {
                    let service_methods: Vec<Value> = methods
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name && row[1] == "service" && row[2] == service[1]
                        })
                        .map(|row| {
                            let params: Vec<Value> = method_params
                                .iter()
                                .filter(|param| {
                                    param[0] == *ctx_name
                                        && param[1] == "service"
                                        && param[2] == service[1]
                                        && param[3] == row[3]
                                })
                                .map(|param| {
                                    json!({
                                        "name": param[4],
                                        "type": param[5],
                                        "required": param[6] == "true",
                                    })
                                })
                                .collect();
                            json!({
                                "name": row[3],
                                "description": row[4],
                                "return_type": row[5],
                                "parameters": params,
                            })
                        })
                        .collect();
                    json!({
                        "name": service[1],
                        "description": service[2],
                        "kind": service[3],
                        "methods": service_methods,
                    })
                })
                .collect();

            let ctx_events: Vec<Value> = events
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|event| {
                    let event_fields: Vec<Value> = fields
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name && row[1] == "event" && row[2] == event[1]
                        })
                        .map(|row| {
                            json!({
                                "name": row[3],
                                "type": row[4],
                                "required": row[5] == "true",
                            })
                        })
                        .collect();
                    json!({
                        "name": event[1],
                        "description": event[2],
                        "source": event[3],
                        "fields": event_fields,
                    })
                })
                .collect();

            let ctx_value_objects: Vec<Value> = value_objects
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|value_object| {
                    let value_object_fields: Vec<Value> = fields
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name
                                && row[1] == "value_object"
                                && row[2] == value_object[1]
                        })
                        .map(|row| {
                            json!({
                                "name": row[3],
                                "type": row[4],
                                "required": row[5] == "true",
                            })
                        })
                        .collect();
                    let rules: Vec<&str> = vo_rules
                        .iter()
                        .filter(|row| row[0] == *ctx_name && row[1] == value_object[1])
                        .map(|row| row[2].as_str())
                        .collect();
                    json!({
                        "name": value_object[1],
                        "description": value_object[2],
                        "fields": value_object_fields,
                        "validation_rules": rules,
                    })
                })
                .collect();

            let ctx_repositories: Vec<Value> = repositories
                .iter()
                .filter(|row| row[0] == *ctx_name)
                .map(|repo| {
                    let repo_methods: Vec<Value> = methods
                        .iter()
                        .filter(|row| {
                            row[0] == *ctx_name && row[1] == "repository" && row[2] == repo[1]
                        })
                        .map(|row| {
                            let params: Vec<Value> = method_params
                                .iter()
                                .filter(|param| {
                                    param[0] == *ctx_name
                                        && param[1] == "repository"
                                        && param[2] == repo[1]
                                        && param[3] == row[3]
                                })
                                .map(|param| {
                                    json!({
                                        "name": param[4],
                                        "type": param[5],
                                        "required": param[6] == "true",
                                    })
                                })
                                .collect();
                            json!({
                                "name": row[3],
                                "description": row[4],
                                "return_type": row[5],
                                "parameters": params,
                            })
                        })
                        .collect();
                    json!({
                        "name": repo[1],
                        "aggregate": repo[2],
                        "methods": repo_methods,
                    })
                })
                .collect();

            json!({
                "name": ctx_name,
                "description": ctx_row[1],
                "module": ctx_row[2],
                "entities": ctx_entities,
                "services": ctx_services,
                "events": ctx_events,
                "value_objects": ctx_value_objects,
                "repositories": ctx_repositories,
                "depends_on": depends_on,
            })
        })
        .collect();

    json!({
        "project": proj_name,
        "description": proj_desc,
        "tech": tech,
        "bounded_contexts": bounded_contexts,
        "rules": rules,
        "conventions": conventions,
    })
}

fn build_rust_ontology_contract_json(store: &Store, workspace: &str, state: &str) -> Value {
    let model = match state {
        "actual" | "implemented" | "current" => store.load_actual(workspace),
        _ => store.load_actual(workspace),
    }
    .ok()
    .flatten();

    let Some(model) = model else {
        return json!({
            "available": false,
            "contract": "Rust facts are the ground truth; no persisted model is available yet.",
            "counts": {
                "source_files": 0,
                "symbols": 0,
                "structs": 0,
                "enums": 0,
                "traits": 0,
                "functions": 0,
                "methods": 0,
                "imports": 0,
                "calls": 0
            }
        });
    };

    let mut symbol_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut method_counts_by_owner: BTreeMap<String, usize> = BTreeMap::new();
    for symbol in &model.symbols {
        *symbol_counts.entry(symbol.kind.clone()).or_default() += 1;
        if symbol.kind == "method"
            && let Some((owner, _)) = symbol.name.split_once("::")
        {
            *method_counts_by_owner.entry(owner.to_string()).or_default() += 1;
        }
    }

    let modules: Vec<Value> = rust_modules_for_contract(&model)
        .iter()
        .map(|(path, file_count)| {
            json!({
                "path": path,
                "kind": if path.contains("::") { "submodule" } else { "module" },
                "file_count": file_count,
            })
        })
        .collect();

    let structs: Vec<Value> = model
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == "struct")
        .map(|symbol| {
            json!({
                "name": symbol.name,
                "module": rust_module_path_from_file_path(&symbol.file_path),
                "file_path": symbol.file_path,
                "start_line": symbol.start_line,
                "end_line": symbol.end_line,
                "visibility": symbol.visibility,
                "method_count": method_counts_by_owner.get(&symbol.name).copied().unwrap_or_default(),
            })
        })
        .collect();

    json!({
        "available": true,
        "contract": "Rust facts are the ground truth. DDD and pattern terms are semantic overlays, not primary storage nodes.",
        "complete_fact_relations": ["source_file", "symbol", "import_edge", "calls_symbol", "ast_edge", "reference_edge"],
        "overview_projection": {
            "nodes": ["crate", "module", "submodule", "struct"],
            "edges": ["contains", "declares", "imports", "calls"],
            "purpose": "Human-scale architecture overview; not a loss of stored MCP facts."
        },
        "counts": {
            "source_files": model.source_files.len(),
            "symbols": model.symbols.len(),
            "structs": symbol_counts.get("struct").copied().unwrap_or_default(),
            "enums": symbol_counts.get("enum").copied().unwrap_or_default(),
            "traits": symbol_counts.get("trait").copied().unwrap_or_default(),
            "functions": symbol_counts.get("function").copied().unwrap_or_default(),
            "methods": symbol_counts.get("method").copied().unwrap_or_default(),
            "imports": model.import_edges.len(),
            "calls": model.call_edges.len(),
        },
        "modules": modules,
        "structs": structs,
        "query_guidance": {
            "overview": "Use architecture/get_model for the Rust ontology summary.",
            "connectivity": "Use impact with dependency_graph, call_graph_callers, call_graph_callees, call_graph_reachability, call_graph_stats, optimization_recommendations, or practice_findings.",
            "deletion": "Use safe_to_delete with module + struct/symbol aliases.",
            "refresh": "Use sync after code changes if the watcher has not already updated the graph."
        }
    })
}

fn rust_modules_for_contract(model: &DomainModel) -> BTreeMap<String, usize> {
    let mut modules = BTreeMap::new();
    for source_file in &model.source_files {
        if let Some(path) = rust_module_path_from_file_path(&source_file.path) {
            *modules.entry(path).or_default() += 1;
        }
    }
    for context in &model.bounded_contexts {
        for module in &context.modules {
            if !module.path.is_empty() {
                modules.entry(module.path.clone()).or_insert(0);
            }
        }
    }
    modules
}

fn rust_module_path_from_file_path(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let start = parts
        .iter()
        .position(|part| *part == "src")
        .map_or(0, |index| index + 1);
    let relative = &parts[start..];
    let file_name = relative.last()?;
    if !file_name.ends_with(".rs") {
        return None;
    }

    let mut segments: Vec<String> = relative[..relative.len().saturating_sub(1)]
        .iter()
        .map(|segment| segment.to_string())
        .collect();
    let stem = file_name.trim_end_matches(".rs");
    if stem != "mod" {
        if (stem == "lib" || stem == "main") && segments.is_empty() {
            return None;
        }
        segments.push(stem.to_string());
    }
    (!segments.is_empty()).then(|| segments.join("::"))
}

fn relation_justification(
    fact_kind: &str,
    fact_state: &str,
    basis_timestamp_us: i64,
) -> ReasoningJustification {
    fact_justification(fact_kind, "*", fact_state, basis_timestamp_us)
}

fn fact_justification(
    fact_kind: &str,
    fact_key: &str,
    fact_state: &str,
    basis_timestamp_us: i64,
) -> ReasoningJustification {
    ReasoningJustification {
        fact_kind: fact_kind.into(),
        fact_key: fact_key.into(),
        fact_state: fact_state.into(),
        basis_timestamp_us,
    }
}

fn actual_relation_justifications(
    basis: &SnapshotBasis,
    relations: &[&str],
) -> Vec<ReasoningJustification> {
    relations
        .iter()
        .map(|relation| relation_justification(relation, "actual", basis.actual_ts))
        .collect()
}

fn default_justifications_for_claim(
    claim: &PersistedReasoningClaim,
    data: &MaterializedReasoningData,
) -> Vec<ReasoningJustification> {
    let mut justifications = match claim.claim_id.as_str() {
        CLAIM_ARCHITECTURE_OVERVIEW => {
            let mut items = actual_relation_justifications(
                &data.basis,
                &[
                    "project",
                    "context",
                    "context_dep",
                    "entity",
                    "service",
                    "event",
                    "value_object",
                    "repository",
                    "module",
                    "field",
                    "method",
                    "method_param",
                    "invariant",
                    "vo_rule",
                ],
            );
            items.extend(actual_relation_justifications(
                &data.basis,
                &[
                    "context",
                    "context_dep",
                    "entity",
                    "service",
                    "event",
                    "value_object",
                    "repository",
                    "module",
                    "source_file",
                    "symbol",
                    "import_edge",
                    "calls_symbol",
                    "ast_edge",
                ],
            ));
            items.push(relation_justification(
                "drift",
                "drift",
                data.basis.drift_ts,
            ));
            items
        }
        CLAIM_CHECK_LAYER_VIOLATIONS | CLAIM_WHY_LAYER_VIOLATIONS => {
            actual_relation_justifications(&data.basis, &["service", "service_dep"])
        }
        CLAIM_CHECK_CIRCULAR_DEPS | CLAIM_WHY_CIRCULAR_DEPS => {
            actual_relation_justifications(&data.basis, &["context_dep"])
        }
        CLAIM_CHECK_AGGREGATE_QUALITY | CLAIM_WHY_AGGREGATE_QUALITY => {
            actual_relation_justifications(&data.basis, &["entity", "invariant"])
        }
        CLAIM_CHECK_ORPHAN_CONTEXTS | CLAIM_WHY_ORPHAN_CONTEXTS => {
            actual_relation_justifications(&data.basis, &["context", "context_dep"])
        }
        CLAIM_CHECK_POLICY_VIOLATIONS | CLAIM_WHY_POLICY_VIOLATIONS => {
            actual_relation_justifications(
                &data.basis,
                &["context_dep", "layer_assignment", "dependency_constraint"],
            )
        }
        CLAIM_CHECK_DRIFT
        | CLAIM_DRIFT_OVERVIEW
        | CLAIM_DIAGNOSE_REFACTOR
        | CLAIM_REFACTOR_PLAN => {
            let mut items = actual_relation_justifications(
                &data.basis,
                &[
                    "context",
                    "context_dep",
                    "entity",
                    "service",
                    "event",
                    "value_object",
                    "repository",
                    "module",
                ],
            );
            items.extend(actual_relation_justifications(
                &data.basis,
                &[
                    "context",
                    "context_dep",
                    "entity",
                    "service",
                    "event",
                    "value_object",
                    "repository",
                    "module",
                    "source_file",
                    "symbol",
                    "import_edge",
                    "calls_symbol",
                    "ast_edge",
                ],
            ));
            items.push(relation_justification(
                "drift",
                "drift",
                data.basis.drift_ts,
            ));
            items
        }
        CLAIM_CHECK_ALL => {
            let mut items = actual_relation_justifications(
                &data.basis,
                &[
                    "context",
                    "context_dep",
                    "entity",
                    "service",
                    "service_dep",
                    "invariant",
                    "layer_assignment",
                    "dependency_constraint",
                ],
            );
            items.extend(actual_relation_justifications(
                &data.basis,
                &[
                    "context",
                    "entity",
                    "service",
                    "event",
                    "value_object",
                    "repository",
                    "module",
                ],
            ));
            items.push(relation_justification(
                "drift",
                "drift",
                data.basis.drift_ts,
            ));
            items
        }
        _ => Vec::new(),
    };

    if claim
        .claim_id
        .starts_with(&format!("{CLAIM_SAFE_TO_DELETE_PREFIX}:"))
    {
        let subject = parse_json_subject(&claim.subject);
        let context = subject["context"].as_str().unwrap_or("");
        let entity = subject["entity"].as_str().unwrap_or("");
        let key = format!("{context}/{entity}");
        justifications.extend(actual_relation_justifications(
            &data.basis,
            &["aggregate_member", "event", "repository"],
        ));
        justifications.extend(actual_relation_justifications(
            &data.basis,
            &["import_edge", "ast_edge", "calls_symbol"],
        ));
        justifications.push(fact_justification(
            "entity",
            &key,
            "actual",
            data.basis.actual_ts,
        ));
        justifications.push(fact_justification(
            "symbol",
            entity,
            "actual",
            data.basis.actual_ts,
        ));
    }

    if claim
        .claim_id
        .starts_with(&format!("{CLAIM_HOW_CONNECTED_PREFIX}:"))
    {
        let subject = parse_json_subject(&claim.subject);
        let from = subject["from"].as_str().unwrap_or("");
        let to = subject["to"].as_str().unwrap_or("");
        justifications.extend(actual_relation_justifications(
            &data.basis,
            &["context", "context_dep"],
        ));
        if !from.is_empty() {
            justifications.push(fact_justification(
                "context",
                from,
                "actual",
                data.basis.actual_ts,
            ));
        }
        if !to.is_empty() {
            justifications.push(fact_justification(
                "context",
                to,
                "actual",
                data.basis.actual_ts,
            ));
        }
    }

    if claim
        .claim_id
        .starts_with(&format!("{CLAIM_IMPACT_PREFIX}:"))
    {
        let subject = parse_json_subject(&claim.subject);
        let analysis = subject["analysis"].as_str().unwrap_or("");
        match analysis {
            "transitive_deps"
            | "circular_deps"
            | "dependency_graph"
            | "pagerank"
            | "community_detection"
            | "betweenness_centrality"
            | "degree_centrality"
            | "topological_order" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["context", "context_dep"],
                ));
            }
            "layer_violations" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["service", "service_dep"],
                ));
            }
            "aggregate_quality" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["entity", "invariant"],
                ));
            }
            "impact_analysis" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["event", "repository", "service_dep", "context_dep"],
                ));
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["ast_edge", "import_edge"],
                ));
            }
            "field_usage" | "shared_fields" => {
                justifications.extend(actual_relation_justifications(&data.basis, &["field"]));
            }
            "method_search" => {
                justifications.extend(actual_relation_justifications(&data.basis, &["method"]));
            }
            "call_graph_callers"
            | "call_graph_callees"
            | "call_graph_reachability"
            | "call_graph_stats" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["calls_symbol"],
                ));
            }
            "optimization_recommendations" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &[
                        "source_file",
                        "symbol",
                        "import_edge",
                        "calls_symbol",
                        "ast_edge",
                    ],
                ));
            }
            "practice_findings" => {
                justifications.extend(actual_relation_justifications(
                    &data.basis,
                    &["symbol", "calls_symbol", "ast_edge", "reference_edge"],
                ));
            }
            _ => {}
        }
        for key in ["context", "entity", "symbol", "field_type", "method_name"] {
            if let Some(value) = subject[key].as_str().filter(|value| !value.is_empty()) {
                justifications.push(fact_justification(
                    key,
                    value,
                    "actual",
                    data.basis.actual_ts,
                ));
            }
        }
    }

    if claim
        .claim_id
        .starts_with(&format!("{CLAIM_HISTORY_PREFIX}:"))
    {
        let subject = parse_json_subject(&claim.subject);
        let raw_state = subject["state"].as_str().unwrap_or("actual");
        let state = if raw_state == "desired" {
            "desired"
        } else {
            "actual"
        };
        let basis_ts = if state == "desired" {
            data.basis.desired_ts
        } else {
            data.basis.actual_ts
        };
        justifications.push(fact_justification("snapshot_log", state, state, basis_ts));
    }

    if claim
        .claim_id
        .starts_with(&format!("{CLAIM_SEARCH_PREFIX}:"))
    {
        justifications.extend(actual_relation_justifications(
            &data.basis,
            &[
                "context",
                "entity",
                "service",
                "event",
                "architectural_decision",
                "invariant",
            ],
        ));
    }

    justifications
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{BoundedContext, Conventions, DomainModel, Ownership, TechStack};
    use std::env::temp_dir;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_store() -> Store {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir().join(format!(
            "axon_reasoning_test_{}_{}.db",
            std::process::id(),
            id
        ));
        Store::open(&path).unwrap()
    }

    fn simple_model() -> DomainModel {
        DomainModel {
            name: "ReasoningProject".into(),
            description: "Kernel test model".into(),
            bounded_contexts: vec![BoundedContext {
                name: "Identity".into(),
                description: "Identity context".into(),
                module_path: "src/identity".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![],
                value_objects: vec![],
                services: vec![],
                repositories: vec![],
                events: vec![],
                modules: vec![],
                dependencies: vec![],
                api_endpoints: vec![],
            }],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![],
            symbols: vec![],
            import_edges: vec![],
            call_edges: vec![],
            reference_edges: vec![],
        }
    }

    fn dependency_model(has_dependency: bool) -> DomainModel {
        DomainModel {
            name: "DependencyProject".into(),
            description: "Connectivity test model".into(),
            bounded_contexts: vec![
                BoundedContext {
                    name: "A".into(),
                    description: "Upstream context".into(),
                    module_path: "src/a".into(),
                    ownership: Ownership::default(),
                    aggregates: vec![],
                    policies: vec![],
                    read_models: vec![],
                    entities: vec![],
                    value_objects: vec![],
                    services: vec![],
                    repositories: vec![],
                    events: vec![],
                    modules: vec![],
                    dependencies: if has_dependency {
                        vec!["B".into()]
                    } else {
                        vec![]
                    },
                    api_endpoints: vec![],
                },
                BoundedContext {
                    name: "B".into(),
                    description: "Downstream context".into(),
                    module_path: "src/b".into(),
                    ownership: Ownership::default(),
                    aggregates: vec![],
                    policies: vec![],
                    read_models: vec![],
                    entities: vec![],
                    value_objects: vec![],
                    services: vec![],
                    repositories: vec![],
                    events: vec![],
                    modules: vec![],
                    dependencies: vec![],
                    api_endpoints: vec![],
                },
            ],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![],
            symbols: vec![],
            import_edges: vec![],
            call_edges: vec![],
            reference_edges: vec![],
        }
    }

    #[test]
    fn test_kernel_materializes_and_loads_claims() {
        let store = test_store();
        let ws = "/tmp/test-reasoning-kernel-materialize";
        let model = simple_model();
        store.save_desired(ws, &model).unwrap();
        store.save_actual(ws, &model).unwrap();
        store.compute_drift(ws).unwrap();

        let kernel = ReasoningKernel::new(&store);
        let claim = kernel.check(ws, "layer_violations").unwrap();
        assert_eq!(claim.claim_id, CLAIM_CHECK_LAYER_VIOLATIONS);
        assert!(!claim.stale);

        let stored = store
            .load_reasoning_claim(ws, CLAIM_CHECK_LAYER_VIOLATIONS)
            .unwrap()
            .unwrap();
        assert_eq!(stored.claim_kind, "check");
        assert_eq!(stored.status, "true");
        assert!(!stored.derivations.is_empty());
    }

    #[test]
    fn test_kernel_dependency_invalidation_is_scoped() {
        let store = test_store();
        let ws = "/tmp/test-reasoning-kernel-invalidation";
        let model = simple_model();
        store.save_desired(ws, &model).unwrap();
        store.save_actual(ws, &model).unwrap();
        store.compute_drift(ws).unwrap();

        let kernel = ReasoningKernel::new(&store);
        kernel.check(ws, "layer_violations").unwrap();
        kernel.drift(ws).unwrap();

        store.save_actual(ws, &model).unwrap();

        let desired_only = store
            .load_reasoning_claim(ws, CLAIM_CHECK_LAYER_VIOLATIONS)
            .unwrap()
            .unwrap();
        assert!(
            desired_only.stale,
            "implemented-graph claim should be invalidated after actual change"
        );

        let drift_claim = store
            .load_reasoning_claim(ws, CLAIM_DRIFT_OVERVIEW)
            .unwrap()
            .unwrap();
        assert!(
            drift_claim.stale,
            "cross-state drift claim should be invalidated after actual change"
        );

        store.compute_drift(ws).unwrap();
        let refreshed = kernel.drift(ws).unwrap();
        assert!(!refreshed.stale);

        store.save_desired(ws, &model).unwrap();
        let invalidated = store
            .load_reasoning_claim(ws, CLAIM_CHECK_LAYER_VIOLATIONS)
            .unwrap()
            .unwrap();
        assert!(
            invalidated.stale,
            "implemented dependency should invalidate implemented-state claim"
        );

        let refreshed_check = kernel.check(ws, "layer_violations").unwrap();
        assert!(!refreshed_check.stale);
    }

    #[test]
    fn test_parameterized_claim_eager_refreshes_after_implemented_change() {
        let store = test_store();
        let ws = "/tmp/test-reasoning-parameterized-refresh";
        let initial = dependency_model(true);
        store.save_desired(ws, &initial).unwrap();
        store.save_actual(ws, &initial).unwrap();
        store.compute_drift(ws).unwrap();

        let kernel = ReasoningKernel::new(&store);
        let initial_claim = kernel.how_connected(ws, "A", "B").unwrap();
        assert_eq!(initial_claim.claim_kind, "how_connected");
        assert_eq!(initial_claim.payload["reachable"], true);

        let updated = dependency_model(false);
        store.save_desired(ws, &updated).unwrap();

        let claim_id = how_connected_claim_id("A", "B");
        let stale = store.load_reasoning_claim(ws, &claim_id).unwrap().unwrap();
        assert!(
            stale.stale,
            "parameterized claim should be invalidated by implemented graph changes"
        );

        let refreshed = kernel.eager_refresh_for_dependency(ws, "actual").unwrap();
        assert!(refreshed.iter().any(|claim| claim.claim_id == claim_id));

        let rebuilt = store.load_reasoning_claim(ws, &claim_id).unwrap().unwrap();
        assert!(!rebuilt.stale);
        assert_eq!(rebuilt.payload["reachable"], false);
        assert_eq!(rebuilt.status, "disconnected");
    }
}
