//! # Graph — travessia de grafos sobre relações entre pontos
//!
//! Implementa BFS usando o campo `relations` de cada ponto para navegar
//! o grafo e obter subconjuntos conectados (ex.: para busca híbrida graph + vetor).

use std::collections::{HashMap, VecDeque};

use crate::error::FerresError;
use crate::point::Point;

/// Executa Breadth-First Search a partir de `start_id` com limite de profundidade.
///
/// **Input:**
/// - `get_point`: função que retorna um ponto pelo seu `storage_id` (ex.: closure sobre a coleção).
/// - `start_id`: ID do nó inicial (storage_id).
/// - `max_depth`: número máximo de saltos (0 = só o nó inicial, 1 = nó + vizinhos diretos, etc.).
///
/// **Output:** Lista de pontos únicos encontrados durante a navegação, em ordem de descoberta (BFS).
///
/// # Erros
/// - `PointNotFound` se `start_id` não existir.
/// - `InvalidPointId` se `start_id` for vazio.
pub fn traverse_bfs<F>(
    get_point: F,
    start_id: &str,
    max_depth: u32,
) -> Result<Vec<Point>, FerresError>
where
    F: Fn(&str) -> Option<Point>,
{
    if start_id.is_empty() {
        return Err(FerresError::InvalidPointId(
            "traverse_bfs: start_id cannot be empty".into(),
        ));
    }

    let start =
        get_point(start_id).ok_or_else(|| FerresError::PointNotFound(start_id.to_string()))?;

    let mut visited: HashMap<String, ()> = HashMap::new();
    let mut result: Vec<Point> = Vec::new();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();

    queue.push_back((start.storage_id(), 0));
    visited.insert(start.storage_id(), ());
    result.push(start);

    while let Some((id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let point = match get_point(&id) {
            Some(p) => p,
            None => continue,
        };

        let next_depth = depth + 1;
        if let Some(ref rels) = point.relations {
            for neighbor_id in rels {
                if neighbor_id.is_empty() {
                    continue;
                }
                if visited.contains_key(neighbor_id) {
                    continue;
                }
                visited.insert(neighbor_id.clone(), ());
                if let Some(neighbor) = get_point(neighbor_id) {
                    result.push(neighbor.clone());
                    queue.push_back((neighbor.storage_id(), next_depth));
                }
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point::Point;

    fn make_point(id: &str, relations: Option<Vec<&str>>) -> Point {
        let mut p = Point::new(id, vec![1.0, 0.0, 0.0], serde_json::Value::Null).unwrap();
        p.relations = relations.map(|v| v.iter().map(|s| (*s).to_string()).collect());
        p
    }

    #[test]
    fn bfs_depth_0_returns_only_start() {
        let get = |id: &str| {
            if id == "a" {
                Some(make_point("a", None))
            } else {
                None
            }
        };
        let points = traverse_bfs(get, "a", 0).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].id, "a");
    }

    #[test]
    fn bfs_depth_1_returns_start_and_neighbors() {
        let get = |id: &str| match id {
            "a" => Some(make_point("a", Some(vec!["b", "c"]))),
            "b" => Some(make_point("b", None)),
            "c" => Some(make_point("c", None)),
            _ => None,
        };
        let points = traverse_bfs(get, "a", 1).unwrap();
        assert_eq!(points.len(), 3);
        let ids: std::collections::HashSet<_> = points.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
    }

    #[test]
    fn bfs_depth_2_returns_two_hops() {
        let get = |id: &str| match id {
            "a" => Some(make_point("a", Some(vec!["b"]))),
            "b" => Some(make_point("b", Some(vec!["c"]))),
            "c" => Some(make_point("c", None)),
            _ => None,
        };
        let points = traverse_bfs(get, "a", 2).unwrap();
        assert_eq!(points.len(), 3);
        let ids: std::collections::HashSet<_> = points.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
    }

    #[test]
    fn bfs_empty_start_id_errors() {
        let get = |_id: &str| None;
        let err = traverse_bfs(get, "", 1).unwrap_err();
        assert!(matches!(err, FerresError::InvalidPointId(_)));
    }

    #[test]
    fn bfs_start_not_found_errors() {
        let get = |_id: &str| None;
        let err = traverse_bfs(get, "missing", 1).unwrap_err();
        assert!(matches!(err, FerresError::PointNotFound(_)));
    }
}
