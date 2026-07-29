// Trait relation graph construction.
/// Trait relation graph definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum EdgeKind {
    Requires,
    Suggests,
    Conflicts,
}

/// A node in the relation graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Node {
    pub source_index: usize,
    pub trait_id: String,
}

/// An edge in the relation graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Edge {
    pub source_index: usize,
    pub source_trait_id: String,
    pub kind: EdgeKind,
    /// Target ref string for requires/suggests; `None` for targetless conflicts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Reference>,
    /// Resolved target trait ID if the target is a `trait:*` ref among loaded
    /// traits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_target_trait_id: Option<String>,
    pub reason: String,
    /// Exact `when` ref strings carried on the edge for exact evaluation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_refs: Vec<Reference>,
}

/// A detected cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Cycle {
    pub path: Vec<String>,
    pub description: String,
}

/// The relation graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Graph {
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<Cycle>,
}

impl Graph {
    pub fn has_cycles(&self) -> bool {
        !self.cycles.is_empty()
    }
}

/// Build a deterministic relation graph from loaded traits.
///
/// Edges are created for requires and suggests entries. Targetless conflicts
/// create self-edges on the declaring trait (for evaluation purposes). Edges
/// with `when` conditions are marked as conditional — the evaluator must check
/// facts before activating them.
pub fn build_graph(traits: &[Trait]) -> Graph {
    let nodes: Vec<Node> = traits
        .iter()
        .enumerate()
        .map(|(i, t)| Node {
            source_index: i,
            trait_id: t.id.as_str().to_string(),
        })
        .collect();

    let id_set: BTreeSet<&str> = traits.iter().map(|t| t.id.as_str()).collect();

    let mut edges = Vec::new();
    for (i, t) in traits.iter().enumerate() {
        let Some(ref rel) = t.relations else {
            continue;
        };

        for entry in &rel.requires {
            let resolved = resolve_trait_target(&entry.target, &id_set);
            edges.push(Edge {
                source_index: i,
                source_trait_id: t.id.as_str().to_string(),
                kind: EdgeKind::Requires,
                target: Some(entry.target.clone()),
                resolved_target_trait_id: resolved,
                reason: entry.reason.clone(),
                when_refs: entry.when.as_slice().to_vec(),
            });
        }
        for entry in &rel.suggests {
            let resolved = resolve_trait_target(&entry.target, &id_set);
            edges.push(Edge {
                source_index: i,
                source_trait_id: t.id.as_str().to_string(),
                kind: EdgeKind::Suggests,
                target: Some(entry.target.clone()),
                resolved_target_trait_id: resolved,
                reason: entry.reason.clone(),
                when_refs: entry.when.as_slice().to_vec(),
            });
        }
        for entry in &rel.conflicts {
            edges.push(Edge {
                source_index: i,
                source_trait_id: t.id.as_str().to_string(),
                kind: EdgeKind::Conflicts,
                target: None,
                resolved_target_trait_id: None,
                reason: entry.reason.clone(),
                when_refs: entry.when.as_slice().to_vec(),
            });
        }
    }

    let cycles = detect_cycles(&edges);

    Graph {
        nodes,
        edges,
        cycles,
    }
}

fn resolve_trait_target(target: &str, id_set: &BTreeSet<&str>) -> Option<String> {
    let Ok(parsed) = Reference::parse(target) else {
        return None;
    };
    if parsed.kind() == Kind::Trait {
        let id = parsed.ref_path().id();
        if id_set.contains(id) {
            return Some(id.to_string());
        }
    }
    None
}

fn detect_cycles(edges: &[Edge]) -> Vec<Cycle> {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        if let Some(ref target_id) = edge.resolved_target_trait_id {
            adjacency
                .entry(edge.source_trait_id.as_str())
                .or_default()
                .push(target_id.as_str());
        }
    }

    let mut cycles = Vec::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut rec_stack: Vec<String> = Vec::new();
    let mut rec_set: BTreeSet<String> = BTreeSet::new();

    let all_nodes: BTreeSet<&str> = adjacency
        .keys()
        .copied()
        .chain(adjacency.values().flatten().copied())
        .collect();

    for &start in &all_nodes {
        let start_s = start.to_string();
        if visited.contains(&start_s) {
            continue;
        }
        dfs_cycle(
            start,
            &adjacency,
            &mut visited,
            &mut rec_stack,
            &mut rec_set,
            &mut cycles,
        );
    }

    cycles
}

fn dfs_cycle(
    node: &str,
    adjacency: &BTreeMap<&str, Vec<&str>>,
    visited: &mut BTreeSet<String>,
    rec_stack: &mut Vec<String>,
    rec_set: &mut BTreeSet<String>,
    cycles: &mut Vec<Cycle>,
) {
    let node_s = node.to_string();
    visited.insert(node_s.clone());
    rec_stack.push(node_s.clone());
    rec_set.insert(node_s.clone());

    if let Some(neighbors) = adjacency.get(node) {
        for &neighbor in neighbors {
            let neighbor_s = neighbor.to_string();
            if !visited.contains(&neighbor_s) {
                dfs_cycle(neighbor, adjacency, visited, rec_stack, rec_set, cycles);
            } else if rec_set.contains(&neighbor_s) {
                let cycle_start = rec_stack.iter().position(|n| n == &neighbor_s);
                if let Some(idx) = cycle_start {
                    let path: Vec<String> = rec_stack[idx..].to_vec();
                    let path_display = path.join(" -> ");
                    cycles.push(Cycle {
                        path,
                        description: format!("cycle detected: {path_display}"),
                    });
                }
            }
        }
    }

    rec_stack.pop();
    rec_set.remove(&node_s);
}
