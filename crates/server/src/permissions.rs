//! # Permissions — modelo de permissões granulares (RBAC)
//!
//! Define recursos, ações e restrições de metadata para controle de acesso
//! granular por coleção. Admins implicitamente têm todas as permissões.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Recurso ao qual a permissão se aplica.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum Resource {
    /// Todas as collections.
    AllCollections,
    /// Collection específica.
    Collection(String),
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resource::AllCollections => write!(f, "*"),
            Resource::Collection(name) => write!(f, "collection:{}", name),
        }
    }
}

/// Ação que pode ser executada.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// search, get, list
    Read,
    /// upsert, delete points
    Write,
    /// create collection
    Create,
    /// delete collection
    Delete,
    /// manage users, save, etc.
    Admin,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Read => write!(f, "read"),
            Action::Write => write!(f, "write"),
            Action::Create => write!(f, "create"),
            Action::Delete => write!(f, "delete"),
            Action::Admin => write!(f, "admin"),
        }
    }
}

/// Restrição opcional baseada em metadata.
///
/// Quando presente em uma permissão, resultados de busca são filtrados
/// automaticamente para incluir apenas documentos que satisfazem a restrição.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataRestriction {
    /// Campo de metadata que deve estar presente.
    pub field: String,
    /// Valores permitidos.
    pub allowed_values: Vec<serde_json::Value>,
}

/// Permissão completa: recurso + ações + restrição opcional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// Recurso ao qual a permissão se aplica.
    pub resource: Resource,
    /// Ações permitidas neste recurso.
    pub actions: Vec<Action>,
    /// Se presente, resultados de busca são filtrados por esta restrição.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_restriction: Option<MetadataRestriction>,
}

/// Resultado da verificação de permissão.
#[derive(Debug, Clone)]
pub enum PermissionResult {
    /// Ação permitida sem restrições.
    Allowed,
    /// Ação permitida, mas com restrição de metadata nos resultados.
    AllowedWithRestriction(MetadataRestriction),
    /// Ação negada.
    Denied(String),
}

impl PermissionResult {
    /// Retorna true se a ação foi permitida (com ou sem restrição).
    pub fn is_allowed(&self) -> bool {
        !matches!(self, PermissionResult::Denied(_))
    }
}

/// Verifica se um conjunto de permissões permite uma ação em um recurso.
///
/// Precedência:
/// 1. Permissão específica para a collection (match exato) tem precedência.
/// 2. Permissão AllCollections funciona como wildcard.
/// 3. Se nenhuma permissão corresponder, a ação é negada.
///
/// Se a permissão que deu match tem `metadata_restriction`, retorna
/// `AllowedWithRestriction` em vez de `Allowed`.
pub fn check_permission(
    permissions: &[Permission],
    resource: &str,
    action: &Action,
) -> PermissionResult {
    // Se não há permissões configuradas (slice vazio), nega tudo.
    // Nota: Admin bypassa esta checagem no nível do extractor/handler.
    if permissions.is_empty() {
        return PermissionResult::Denied(format!(
            "no permissions configured for action '{}' on '{}'",
            action, resource
        ));
    }

    // Primeiro, procura match específico (Collection(name))
    for perm in permissions {
        let matches_resource = match &perm.resource {
            Resource::Collection(name) => {
                name == resource || resource == format!("collection:{}", name)
            }
            Resource::AllCollections => false, // verificado no segundo passo
        };

        if matches_resource && perm.actions.contains(action) {
            return match &perm.metadata_restriction {
                Some(restriction) => {
                    PermissionResult::AllowedWithRestriction(restriction.clone())
                }
                None => PermissionResult::Allowed,
            };
        }
    }

    // Segundo, procura wildcard (AllCollections)
    for perm in permissions {
        if perm.resource == Resource::AllCollections && perm.actions.contains(action) {
            return match &perm.metadata_restriction {
                Some(restriction) => {
                    PermissionResult::AllowedWithRestriction(restriction.clone())
                }
                None => PermissionResult::Allowed,
            };
        }
    }

    PermissionResult::Denied(format!(
        "permission denied: action '{}' not allowed on '{}'",
        action, resource
    ))
}

/// Converte um filtro de MetadataRestriction para um serde_json::Value de filtro
/// compatível com MetadataFilter::from_json (formato $in).
///
/// Usado para injetar automaticamente no filtro de busca quando o usuário
/// tem MetadataRestriction.
pub fn restriction_to_filter(restriction: &MetadataRestriction) -> serde_json::Value {
    serde_json::json!({
        restriction.field.clone(): {
            "$in": restriction.allowed_values.clone()
        }
    })
}

/// Mescla um filtro de restrição com um filtro existente do request (AND).
///
/// Se não há filtro existente, retorna apenas o filtro de restrição.
/// Se há filtro existente (object), adiciona/sobrescreve o campo da restrição.
pub fn merge_restriction_filter(
    existing_filter: Option<&serde_json::Value>,
    restriction: &MetadataRestriction,
) -> serde_json::Value {
    let restriction_filter = restriction_to_filter(restriction);

    match existing_filter {
        None | Some(serde_json::Value::Null) => restriction_filter,
        Some(existing) => {
            if let Some(existing_obj) = existing.as_object() {
                let mut merged = existing_obj.clone();
                // O filtro de restrição tem precedência (AND semântico:
                // o campo da restrição é sobrescrito para garantir enforcement)
                if let Some(restriction_obj) = restriction_filter.as_object() {
                    for (k, v) in restriction_obj {
                        merged.insert(k.clone(), v.clone());
                    }
                }
                serde_json::Value::Object(merged)
            } else {
                // Se o filtro existente não é um object, usa apenas a restrição
                restriction_filter
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_action_on_specific_collection() {
        let perms = vec![Permission {
            resource: Resource::Collection("docs".to_string()),
            actions: vec![Action::Read, Action::Write],
            metadata_restriction: None,
        }];

        let result = check_permission(&perms, "docs", &Action::Read);
        assert!(result.is_allowed());

        let result = check_permission(&perms, "docs", &Action::Write);
        assert!(result.is_allowed());

        let result = check_permission(&perms, "docs", &Action::Delete);
        assert!(!result.is_allowed());

        let result = check_permission(&perms, "other", &Action::Read);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_all_collections_wildcard() {
        let perms = vec![Permission {
            resource: Resource::AllCollections,
            actions: vec![Action::Read],
            metadata_restriction: None,
        }];

        let result = check_permission(&perms, "docs", &Action::Read);
        assert!(result.is_allowed());

        let result = check_permission(&perms, "other", &Action::Read);
        assert!(result.is_allowed());

        let result = check_permission(&perms, "docs", &Action::Write);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_metadata_restriction() {
        let perms = vec![Permission {
            resource: Resource::AllCollections,
            actions: vec![Action::Read],
            metadata_restriction: Some(MetadataRestriction {
                field: "department".to_string(),
                allowed_values: vec![serde_json::json!("sales")],
            }),
        }];

        let result = check_permission(&perms, "docs", &Action::Read);
        match result {
            PermissionResult::AllowedWithRestriction(r) => {
                assert_eq!(r.field, "department");
                assert_eq!(r.allowed_values, vec![serde_json::json!("sales")]);
            }
            _ => panic!("expected AllowedWithRestriction"),
        }
    }

    #[test]
    fn test_empty_permissions_denies() {
        let perms: Vec<Permission> = vec![];
        let result = check_permission(&perms, "docs", &Action::Read);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_specific_overrides_wildcard() {
        let perms = vec![
            Permission {
                resource: Resource::AllCollections,
                actions: vec![Action::Read],
                metadata_restriction: Some(MetadataRestriction {
                    field: "department".to_string(),
                    allowed_values: vec![serde_json::json!("sales")],
                }),
            },
            Permission {
                resource: Resource::Collection("public".to_string()),
                actions: vec![Action::Read],
                metadata_restriction: None,
            },
        ];

        // "public" collection: specific match, no restriction
        let result = check_permission(&perms, "public", &Action::Read);
        assert!(matches!(result, PermissionResult::Allowed));

        // "docs" collection: wildcard match, has restriction
        let result = check_permission(&perms, "docs", &Action::Read);
        assert!(matches!(result, PermissionResult::AllowedWithRestriction(_)));
    }

    #[test]
    fn test_merge_restriction_filter_no_existing() {
        let restriction = MetadataRestriction {
            field: "department".to_string(),
            allowed_values: vec![serde_json::json!("sales")],
        };

        let merged = merge_restriction_filter(None, &restriction);
        assert_eq!(
            merged,
            serde_json::json!({"department": {"$in": ["sales"]}})
        );
    }

    #[test]
    fn test_merge_restriction_filter_with_existing() {
        let restriction = MetadataRestriction {
            field: "department".to_string(),
            allowed_values: vec![serde_json::json!("sales")],
        };
        let existing = serde_json::json!({"category": "tech"});

        let merged = merge_restriction_filter(Some(&existing), &restriction);
        let obj = merged.as_object().unwrap();
        assert!(obj.contains_key("category"));
        assert!(obj.contains_key("department"));
    }

    #[test]
    fn test_permission_serialization() {
        let perm = Permission {
            resource: Resource::Collection("docs".to_string()),
            actions: vec![Action::Read, Action::Write],
            metadata_restriction: None,
        };
        let json = serde_json::to_string(&perm).unwrap();
        let deserialized: Permission = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.resource, perm.resource);
        assert_eq!(deserialized.actions, perm.actions);
    }
}
