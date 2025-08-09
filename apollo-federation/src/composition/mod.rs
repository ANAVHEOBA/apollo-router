mod satisfiability;

use std::vec;

pub use crate::composition::satisfiability::validate_satisfiability;
use crate::error::CompositionError;
pub use crate::schema::schema_upgrader::upgrade_subgraphs_if_necessary;
use crate::subgraph::typestate::{Expanded, Initial, Subgraph, Upgraded, Validated};
use crate::supergraph::{Merged, Satisfiable, Supergraph};

/* ---------- public entry point ---------- */

pub fn compose(
    subgraphs: Vec<Subgraph<Initial>>,
) -> Result<Supergraph<Satisfiable>, Vec<CompositionError>> {
    let expanded_subgraphs = expand_subgraphs(subgraphs)?;
    let upgraded_subgraphs = upgrade_subgraphs_if_necessary(expanded_subgraphs)?;
    let validated_subgraphs = validate_subgraphs(upgraded_subgraphs)?;

    pre_merge_validations(&validated_subgraphs)?;
    let supergraph = merge_subgraphs(validated_subgraphs)?;
    post_merge_validations(&supergraph)?;
    validate_satisfiability(supergraph)
}

/* ---------- helpers ---------- */

/// Expand subgraph links (add missing federation definitions)
pub fn expand_subgraphs(
    subgraphs: Vec<Subgraph<Initial>>,
) -> Result<Vec<Subgraph<Expanded>>, Vec<CompositionError>> {
    let mut errors = Vec::new();
    let expanded = subgraphs
        .into_iter()
        .map(|s| s.expand_links())
        .filter_map(|r| r.map_err(|e| errors.push(e.into())).ok())
        .collect();
    if errors.is_empty() {
        Ok(expanded)
    } else {
        Err(errors)
    }
}

/// Validate each subgraph against the Federation spec
pub fn validate_subgraphs(
    subgraphs: Vec<Subgraph<Upgraded>>,
) -> Result<Vec<Subgraph<Validated>>, Vec<CompositionError>> {
    let mut errors = Vec::new();
    let validated = subgraphs
        .into_iter()
        .map(|s| s.validate())
        .filter_map(|r| r.map_err(|e| errors.push(e.into())).ok())
        .collect();
    if errors.is_empty() {
        Ok(validated)
    } else {
        Err(errors)
    }
}

/* ---------- validation before merge ---------- */

pub fn pre_merge_validations(
    subgraphs: &[Subgraph<Validated>],
) -> Result<(), Vec<CompositionError>> {
    let mut errors = Vec::new();

    // 1. Duplicate names
    let mut seen = std::collections::HashSet::new();
    for s in subgraphs {
        if !seen.insert(&s.name) {
            errors.push(CompositionError::InternalError {
                message: format!("Duplicate subgraph name: {}", s.name),
            });
        }
    }

    // 2. Conflicting root types
    let mut roots = std::collections::HashMap::<&'static str, apollo_compiler::Name>::new();
    for s in subgraphs {
        let schema = s.schema();
        for (kind, root) in &[
            ("Query", &schema.schema().schema_definition.query),
            ("Mutation", &schema.schema().schema_definition.mutation),
            ("Subscription", &schema.schema().schema_definition.subscription),
        ] {
            if let Some(name) = root {
                match roots.entry(kind) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        // Fix 1: Convert ComponentName to Name for insertion
                        e.insert(name.name.clone());
                    }
                    std::collections::hash_map::Entry::Occupied(e) => {
                        // Fix 2: Compare Name with ComponentName's name field
                        if *e.get() != name.name {
                            errors.push(CompositionError::InternalError {
                                message: format!(
                                    "Conflicting {} root type in subgraph: {}",
                                    kind, s.name
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/* ---------- merge subgraphs into supergraph ---------- */

pub fn merge_subgraphs(
    subgraphs: Vec<Subgraph<Validated>>,
) -> Result<Supergraph<Merged>, Vec<CompositionError>> {
    use crate::merger::merge::{merge_subgraphs as do_merge, CompositionOptions};

    let result = do_merge(subgraphs, CompositionOptions::default())
        .map_err(|e| vec![CompositionError::InternalError {
            message: e.to_string(),
        }])?;

    let schema = result
        .supergraph
        .map(|federation_schema| {
            // Convert Valid<FederationSchema> to Valid<Schema>
            let inner_schema = federation_schema.into_inner().into_inner();
            inner_schema.validate().expect("schema should be valid")
        })
        .unwrap_or_else(|| {
            // Create empty schema and validate it to get Valid<Schema>
            let empty_schema = apollo_compiler::Schema::new();
            empty_schema.validate().expect("empty schema is valid")
        });

    // Use turbofish syntax to disambiguate
    Ok(Supergraph::<Merged>::new(schema))
}

/* ---------- final validation of merged schema ---------- */

pub fn post_merge_validations(
    supergraph: &Supergraph<Merged>,
) -> Result<(), Vec<CompositionError>> {
    // Get the inner Schema from Valid<Schema>
    let schema = supergraph.state.schema().clone().into_inner();
    // Clone the Schema before validating it
    let cloned_schema = schema.clone();
    cloned_schema.validate()
        .map_err(|e| vec![CompositionError::InternalError {
            message: e.to_string(),
        }])?;
    Ok(())
}