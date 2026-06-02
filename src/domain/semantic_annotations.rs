//! Semantic overlay vocabulary layered on top of Rust facts.
//!
//! These types describe domain, ownership, policy, and design intent. They are
//! annotations over the Rust fact graph rather than a substitute for it.

pub use super::model::{
    APIEndpoint, Aggregate, ArchitecturalDecision, ArchitecturalRule, BoundedContext, Conventions,
    DecisionStatus, DomainEvent, Entity, ExternalSystem, Field, FileStructure, Method, Module,
    NamingConventions, Ownership, Policy, PolicyKind, ReadModel, Repository, Service, ServiceKind,
    Severity, TechStack, ValueObject,
};
