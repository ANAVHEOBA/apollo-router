mod satisfiability;

use std::vec;

pub use crate::composition::satisfiability::validate_satisfiability;
use crate::error::CompositionError;
pub use crate::schema::schema_upgrader::upgrade_subgraphs_if_necessary;
use crate::subgraph::typestate::{Expanded, Initial, Subgraph, Upgraded, Validated};
use crate::supergraph::{Merged, Satisfiable, Supergraph};
use crate::schema::validators::key::validate_key_directives;
use crate::schema::validators::shareable::validate_shareable_directives;
use crate::link::federation_spec_definition::FEDERATION_EXTENDS_DIRECTIVE_NAME_IN_SPEC;
use crate::link::spec_definition::SpecDefinition;
use crate::schema::ValidFederationSchema;
use crate::schema::HasFields;
use apollo_compiler::validation::Valid;
use crate::schema::validators::external::validate_external_directives;

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
                        e.insert(name.name.clone());
                    }
                    std::collections::hash_map::Entry::Occupied(e) => {
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

    // 3. Validate @key directives for entity types
    validate_key_directives_for_entities(subgraphs, &mut errors)?;
    
    // 4. Validate @shareable directive consistency
    validate_shareable_consistency(subgraphs, &mut errors)?;
    
    // 5. Validate interface implementation consistency
    validate_interface_implementation_consistency(subgraphs, &mut errors)?;
    
    // 6. Validate @extends directives
    validate_extends_directives(subgraphs, &mut errors)?;

    // 7. Validate @external directives
    validate_external_directives_consistency(subgraphs, &mut errors)?;

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// Validate that entity types have @key directives
fn validate_key_directives_for_entities(
    subgraphs: &[Subgraph<Validated>],
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        let metadata = subgraph.metadata();
        
        let mut key_errors = crate::error::MultipleFederationErrors::new();
        if let Err(e) = validate_key_directives(schema, metadata, &mut key_errors) {
            errors.push(CompositionError::InternalError {
                message: e.to_string(),
            });
        }
        
        for error in key_errors.errors {
            errors.push(CompositionError::InternalError {
                message: error.to_string(),
            });
        }
    }
    Ok(())
}

// Validate @shareable directive consistency across subgraphs
fn validate_shareable_consistency(
    subgraphs: &[Subgraph<Validated>],
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        let metadata = subgraph.metadata();
        
        let mut shareable_errors = crate::error::MultipleFederationErrors::new();
        if let Err(e) = validate_shareable_directives(schema, metadata, &mut shareable_errors) {
            errors.push(CompositionError::InternalError {
                message: e.to_string(),
            });
        }
        
        for error in shareable_errors.errors {
            errors.push(CompositionError::InternalError {
                message: error.to_string(),
            });
        }
    }
    Ok(())
}

// Validate @external directives consistency across subgraphs
fn validate_external_directives_consistency(
    subgraphs: &[Subgraph<Validated>],
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        let metadata = subgraph.metadata();
        
        let mut external_errors = crate::error::MultipleFederationErrors::new();
        if let Err(e) = validate_external_directives(schema, metadata, &mut external_errors) {
            errors.push(CompositionError::InternalError {
                message: e.to_string(),
            });
        }
        
        for error in external_errors.errors {
            errors.push(CompositionError::InternalError {
                message: error.to_string(),
            });
        }
    }
    Ok(())
}

// Validate interface implementation consistency across subgraphs
fn validate_interface_implementation_consistency(
    subgraphs: &[Subgraph<Validated>],
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    let mut interface_implementations: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();
    
    // Collect all interface implementations from all subgraphs
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        for (type_name, type_def) in &schema.schema().types {
            if let apollo_compiler::schema::ExtendedType::Object(obj) = type_def {
                for interface_name in &obj.implements_interfaces {
                    let implementations = interface_implementations
                        .entry(interface_name.to_string())
                        .or_insert_with(std::collections::HashSet::new);
                    implementations.insert(type_name.to_string());
                }
            }
        }
    }
    
    // Check for consistency across subgraphs
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        let mut current_implementations: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();
        
        for (type_name, type_def) in &schema.schema().types {
            if let apollo_compiler::schema::ExtendedType::Object(obj) = type_def {
                for interface_name in &obj.implements_interfaces {
                    let implementations = current_implementations
                        .entry(interface_name.to_string())
                        .or_insert_with(std::collections::HashSet::new);
                    implementations.insert(type_name.to_string());
                }
            }
        }
        
        // Compare with expected implementations
        for (interface_name, expected_implementations) in &interface_implementations {
            let actual_implementations = current_implementations
                .get(interface_name)
                .cloned()
                .unwrap_or_default();
            
            if actual_implementations != *expected_implementations {
                let missing: Vec<_> = expected_implementations
                    .difference(&actual_implementations)
                    .cloned()
                    .collect();
                let extra: Vec<_> = actual_implementations
                    .difference(expected_implementations)
                    .cloned()
                    .collect();
                
                if !missing.is_empty() || !extra.is_empty() {
                    errors.push(CompositionError::InternalError {
                        message: format!(
                            "Interface {} has inconsistent implementations in subgraph {}. Missing: {:?}, Extra: {:?}",
                            interface_name, subgraph.name, missing, extra
                        ),
                    });
                }
            }
        }
    }
    
    Ok(())
}

// Validate @extends directives must reference existing types
fn validate_extends_directives(
    subgraphs: &[Subgraph<Validated>],
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    // Check for @extends directives by looking for types with @extends directive
    for subgraph in subgraphs {
        let schema = subgraph.schema();
        let metadata = subgraph.metadata();
        
        // Get the extends directive name for this subgraph
        let extends_directive_name = match metadata.federation_spec_definition().directive_name_in_schema(
            schema,
            &FEDERATION_EXTENDS_DIRECTIVE_NAME_IN_SPEC
        ) {
            Ok(Some(name)) => name,
            _ => continue, // No extends directive in this subgraph
        };
        
        // Check all types that have @extends directive
        let referencers = schema.referencers();
        if let Ok(extends_referencers) = referencers.get_directive(&extends_directive_name) {
            for type_pos in extends_referencers.object_types.iter() {
                let type_name = type_pos.type_name.clone();
                
                // Check if this type exists in other subgraphs
                let mut exists_in_other_subgraphs = false;
                for other_subgraph in subgraphs {
                    if other_subgraph.name != subgraph.name {
                        let other_schema = other_subgraph.schema();
                        if other_schema.schema().types.contains_key(&type_name) {
                            exists_in_other_subgraphs = true;
                            break;
                        }
                    }
                }
                
                if !exists_in_other_subgraphs {
                    errors.push(CompositionError::InternalError {
                        message: format!(
                            "Type {} marked with @extends in subgraph {} must be defined in at least one other subgraph",
                            type_name, subgraph.name
                        ),
                    });
                }
            }
        }
    }
    
    Ok(())
}

/* ---------- merge subgraphs into supergraph ---------- */

pub fn merge_subgraphs(
    subgraphs: Vec<Subgraph<Validated>>,
) -> Result<Supergraph<Merged>, Vec<CompositionError>> {
    use crate::merger::merge::merge_subgraphs;
    use crate::merger::merge::CompositionOptions;

    let result = merge_subgraphs(subgraphs, CompositionOptions::default())
        .map_err(|e| vec![CompositionError::InternalError {
            message: e.to_string(),
        }])?;

    // Check if there are composition errors
    if !result.errors.is_empty() {
        return Err(result.errors);
    }

    // Get the supergraph schema
    let federation_schema = result.supergraph.ok_or_else(|| {
        vec![CompositionError::InternalError {
            message: "Merging succeeded but no supergraph was produced".to_string(),
        }]
    })?;

    // Convert Valid<FederationSchema> to Valid<Schema>
    let inner_schema = federation_schema.into_inner().into_inner();
    let validated_schema = inner_schema.validate().map_err(|e| {
        vec![CompositionError::InternalError {
            message: e.to_string(),
        }]
    })?;

    // Create and return the Supergraph
    Ok(Supergraph::<Merged>::new(validated_schema))
}

/* ---------- final validation of merged schema ---------- */

pub fn post_merge_validations(
    supergraph: &Supergraph<Merged>,
) -> Result<(), Vec<CompositionError>> {
    let mut errors = Vec::new();

    // 1. Basic schema validation
    let schema = supergraph.state.schema().clone().into_inner();
    schema.validate()
        .map_err(|e| vec![CompositionError::InternalError {
            message: e.to_string(),
        }])?;

    // 2. Create ValidFederationSchema for advanced validations
    let fed_schema = ValidFederationSchema::new(supergraph.state.schema().clone())
        .map_err(|e| vec![CompositionError::InternalError {
            message: e.to_string(),
        }])?;

    // 3. Validate merged entity types (@key fields must exist)
    validate_merged_entity_types(&fed_schema, &mut errors)?;

    // 4. Validate interface implementations in the supergraph
    validate_merged_interface_implementations(&fed_schema, &mut errors)?;

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_merged_entity_types(
    schema: &ValidFederationSchema,
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    // Validate that all entity types have valid @key directives
    let key_directives = match schema.key_directive_applications() {
        Ok(directives) => directives,
        Err(e) => {
            errors.push(CompositionError::InternalError {
                message: e.to_string(),
            });
            return Ok(());
        }
    };

    for key_result in key_directives {
        match key_result {
            Ok(key) => {
                // Check if the key fields actually exist in the type
                let fields = match key.parse_fields(schema.schema()) {
                    Ok(fields) => fields,
                    Err(e) => {
                        errors.push(CompositionError::InternalError {
                            message: format!("Failed to parse key fields: {}", e),
                        });
                        continue;
                    }
                };
                
                if let Err(validation_error) = fields.validate(Valid::assume_valid_ref(schema.schema())) {
                    errors.push(CompositionError::InternalError {
                        message: format!("Invalid @key fields: {}", validation_error),
                    });
                }
            }
            Err(e) => {
                errors.push(CompositionError::InternalError {
                    message: e.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn validate_merged_interface_implementations(
    schema: &ValidFederationSchema,
    errors: &mut Vec<CompositionError>,
) -> Result<(), Vec<CompositionError>> {
    use crate::schema::position::InterfaceTypeDefinitionPosition;

    let mut interface_implementations: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    // Collect interface implementations
    for type_pos in schema.get_types() {
        if let Ok(interface_pos) = InterfaceTypeDefinitionPosition::try_from(type_pos.clone()) {
            let implementations = match schema.possible_runtime_types(interface_pos.into()) {
                Ok(impls) => impls,
                Err(e) => {
                    errors.push(CompositionError::InternalError {
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            
            interface_implementations.insert(
                type_pos.type_name().to_string(),
                implementations.iter().map(|t| t.to_string()).collect()
            );
        }
    }

    // Validate that interfaces have consistent implementations
    for (interface_name, implementations) in interface_implementations {
        if implementations.is_empty() {
            errors.push(CompositionError::InternalError {
                message: format!("Interface {} has no implementations", interface_name),
            });
        }
    }

    Ok(())
}