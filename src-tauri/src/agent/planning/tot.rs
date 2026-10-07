//! Tree-of-Thoughts (ToT) reasoning engine.
//!
//! Issue #060: Parallel branch exploration for complex multi-path tasks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::agent::tool::ToolCall;

#[allow(unused_imports)]
use crate::llm::LlmProvider;

/// Status of a ToT node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Pending,
    Active,
    Completed,
    Pruned,
}

/// A single thought node in the ToT exploration tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToTNode {
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub depth: usize,
    pub thought: String,
    #[serde(skip_serializing, deserialize_with = "skip_optional_tool_call")]
    pub action: Option<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    pub score: f32,
    pub status: NodeStatus,
    pub is_terminal: bool,
}

impl ToTNode {
    pub fn new(node_id: String, parent_id: Option<String>, depth: usize) -> Self {
        Self {
            node_id,
            parent_id,
            depth,
            thought: String::new(),
            action: None,
            outcome: None,
            score: 0.5,
            status: NodeStatus::Pending,
            is_terminal: false,
        }
    }

    pub fn branch_path<'a>(&'a self, tree: &'a ToTTree) -> Vec<&'a ToTNode> {
        let mut path = vec![self];
        let mut current = self;
        loop {
            let pid = match &current.parent_id {
                Some(p) => p.clone(),
                None => break,
            };
            match tree.nodes.get(&pid) {
                Some(parent) => {
                    path.push(parent);
                    current = parent;
                }
                None => break,
            }
        }
        path.reverse();
        path
    }
}

/// Helper for skipping ToolCall serialization (does not implement Serialize).
pub fn skip_optional_tool_call<'de, D>(deserializer: D) -> Result<Option<ToolCall>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<()>::deserialize(deserializer).map(|_| None)
}

/// The complete ToT exploration tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToTTree {
    pub root_id: String,
    #[serde(default)]
    pub nodes: HashMap<String, ToTNode>,
    #[serde(default)]
    pub best_path: Vec<String>,
    pub beam_width: usize,
    pub max_depth: usize,
    pub prune_threshold: f32,
}

impl ToTTree {
    pub fn new(root_id: String) -> Self {
        Self {
            root_id,
            nodes: HashMap::new(),
            best_path: Vec::new(),
            beam_width: 3,
            max_depth: 10,
            prune_threshold: 0.3,
        }
    }

    pub fn add_node(&mut self, node: ToTNode) {
        self.nodes.insert(node.node_id.clone(), node);
    }

    pub fn get(&self, node_id: &str) -> Option<&ToTNode> {
        self.nodes.get(node_id)
    }

    pub fn get_mut(&mut self, node_id: &str) -> Option<&mut ToTNode> {
        self.nodes.get_mut(node_id)
    }

    pub fn active_nodes_at_depth(&self, depth: usize) -> Vec<&ToTNode> {
        self.nodes
            .values()
            .filter(|n| n.depth == depth && n.status == NodeStatus::Active)
            .collect()
    }

    pub fn completed_nodes(&self) -> Vec<&ToTNode> {
        self.nodes
            .values()
            .filter(|n| n.status == NodeStatus::Completed)
            .collect()
    }

    pub fn update_best_path(&mut self) {
        let mut completed: Vec<&ToTNode> = self.completed_nodes();
        // Sort by score descending, then by depth descending (prefer deeper paths for ties)
        completed.sort_by(|a, b| {
            let score_cmp = b
                .score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal);
            if score_cmp == std::cmp::Ordering::Equal {
                b.depth.cmp(&a.depth)
            } else {
                score_cmp
            }
        });

        self.best_path = if let Some(best) = completed.first() {
            best.branch_path(self)
                .iter()
                .map(|n| n.node_id.clone())
                .collect()
        } else {
            Vec::new()
        };
    }

    pub fn best_branch(&self) -> Vec<String> {
        self.best_path.clone()
    }

    pub fn best_score(&self) -> f32 {
        self.completed_nodes()
            .iter()
            .map(|n| n.score)
            .fold(0.0f32, |a, b| a.max(b))
    }
}

/// Trigger conditions for ToT activation.
#[derive(Debug, Clone)]
pub struct ToTTrigger {
    pub user_keyword: bool,
    pub multi_path: bool,
    pub repeated_failure: bool,
}

impl ToTTrigger {
    pub fn should_trigger(&self) -> bool {
        self.user_keyword || self.multi_path || self.repeated_failure
    }
}

/// Tree-of-Thoughts reasoning engine.
pub struct ToTEngine {
    pub tree: ToTTree,
    llm_provider: Option<Box<dyn LlmProvider>>,
    expanded_count: usize,
    pruned_count: usize,
}

impl ToTEngine {
    pub fn new(root_id: String) -> Self {
        Self {
            tree: ToTTree::new(root_id),
            llm_provider: None,
            expanded_count: 0,
            pruned_count: 0,
        }
    }

    pub fn with_llm_provider(mut self, provider: Box<dyn LlmProvider>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    pub fn init(&mut self, initial_thought: String) {
        let root = ToTNode {
            node_id: self.tree.root_id.clone(),
            parent_id: None,
            depth: 0,
            thought: initial_thought,
            action: None,
            outcome: None,
            score: 0.5,
            status: NodeStatus::Active,
            is_terminal: false,
        };
        self.tree.add_node(root);
    }

    pub fn expand_branch(
        &mut self,
        parent_id: &str,
        node_id: String,
        thought: String,
        action: Option<ToolCall>,
        outcome: Option<String>,
    ) {
        let parent = match self.tree.get(parent_id) {
            Some(p) => p,
            None => return,
        };

        let depth = parent.depth + 1;
        let is_terminal = depth >= self.tree.max_depth;

        let node = ToTNode {
            node_id: node_id.clone(),
            parent_id: Some(parent_id.to_string()),
            depth,
            thought,
            action,
            outcome,
            score: 0.5,
            status: if is_terminal {
                NodeStatus::Completed
            } else {
                NodeStatus::Active
            },
            is_terminal,
        };

        self.tree.add_node(node);
        self.expanded_count += 1;
    }

    /// Score all active branches and prune those beyond beam_width threshold.
    /// Returns the IDs of pruned nodes.
    pub fn score_and_prune(&mut self) -> Vec<String> {
        let beam_width = self.tree.beam_width;
        let mut pruned = Vec::new();

        // Collect active non-root nodes for pruning evaluation.
        // The root node is never pruned.
        let mut candidates: Vec<(String, f32)> = self
            .tree
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Active && n.node_id != self.tree.root_id)
            .map(|n| {
                let branch_score = self.compute_branch_score(&n.node_id);
                (n.node_id.clone(), branch_score)
            })
            .collect();

        // Sort by score descending, then node_id ascending for deterministic ordering
        // when scores are equal. This ensures beam_width top candidates are always kept.
        candidates.sort_by(|a, b| {
            let score_cmp = b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal);
            if score_cmp == std::cmp::Ordering::Equal {
                a.0.cmp(&b.0)
            } else {
                score_cmp
            }
        });

        // Only prune candidates beyond beam_width
        let to_prune: Vec<String> = candidates
            .into_iter()
            .skip(beam_width)
            .map(|(id, _)| id)
            .collect();

        for node_id in &to_prune {
            if let Some(node) = self.tree.get_mut(node_id) {
                node.status = NodeStatus::Pruned;
                self.pruned_count += 1;
            }
        }

        // Mark nodes at max_depth as completed/terminal
        for node in self.tree.nodes.values_mut() {
            if node.status == NodeStatus::Active && node.depth >= self.tree.max_depth {
                node.status = NodeStatus::Completed;
                node.is_terminal = true;
            }
        }

        pruned.extend(to_prune);
        self.tree.update_best_path();
        pruned
    }

    fn compute_branch_score(&self, node_id: &str) -> f32 {
        let node = match self.tree.get(node_id) {
            Some(n) => n,
            None => return 0.0,
        };

        let path = node.branch_path(&self.tree);
        if path.is_empty() {
            return 0.0;
        }

        let sum: f32 = path.iter().map(|n| n.score).sum();
        sum / path.len() as f32
    }

    pub fn update_score(&mut self, node_id: &str, new_score: f32) {
        if let Some(node) = self.tree.get_mut(node_id) {
            node.score = new_score;
        }
    }

    pub fn complete_node(&mut self, node_id: &str, final_score: f32) {
        if let Some(node) = self.tree.get_mut(node_id) {
            node.score = final_score;
            node.status = NodeStatus::Completed;
        }
        self.tree.update_best_path();
    }

    pub fn should_continue(&self) -> bool {
        !self
            .tree
            .active_nodes_at_depth(self.tree.max_depth - 1)
            .is_empty()
            || (self.expanded_count < self.tree.beam_width * self.tree.max_depth
                && self.pruned_count < self.tree.beam_width * 2)
    }

    pub fn stats(&self) -> ToTStats {
        let total = self.tree.nodes.len();
        let active = self
            .tree
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Active)
            .count();
        let completed = self
            .tree
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Completed)
            .count();

        ToTStats {
            total_nodes: total,
            active_nodes: active,
            completed_nodes: completed,
            pruned_nodes: self.pruned_count,
            best_score: self.tree.best_score(),
        }
    }

    pub fn root(&self) -> Option<&ToTNode> {
        self.tree.get(&self.tree.root_id)
    }

    pub fn all_nodes(&self) -> Vec<&ToTNode> {
        self.tree.nodes.values().collect()
    }

    pub fn should_trigger_tot(user_message: &str, repeated_failure: bool) -> ToTTrigger {
        let msg_lower = user_message.to_lowercase();
        let user_keyword = msg_lower.contains("explore")
            || msg_lower.contains("alternatives")
            || msg_lower.contains("??")
            || msg_lower.contains("??")
            || msg_lower.contains("options");

        ToTTrigger {
            user_keyword,
            multi_path: false,
            repeated_failure,
        }
    }

    pub fn to_frontend_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rootId": self.tree.root_id,
            "bestPath": self.tree.best_branch(),
            "beamWidth": self.tree.beam_width,
            "maxDepth": self.tree.max_depth,
            "stats": {
                "totalNodes": self.stats().total_nodes,
                "activeNodes": self.stats().active_nodes,
                "completedNodes": self.stats().completed_nodes,
                "prunedNodes": self.stats().pruned_nodes,
                "bestScore": self.stats().best_score,
            },
            "nodes": self.all_nodes().iter().map(|n| {
                serde_json::json!({
                    "id": n.node_id,
                    "parentId": n.parent_id,
                    "depth": n.depth,
                    "thought": n.thought,
                    "score": n.score,
                    "status": format!("{:?}", n.status).to_lowercase(),
                    "isTerminal": n.is_terminal,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToTStats {
    pub total_nodes: usize,
    pub active_nodes: usize,
    pub completed_nodes: usize,
    pub pruned_nodes: usize,
    pub best_score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tot_tree_creation() {
        let tree = ToTTree::new("root-1".to_string());
        assert_eq!(tree.root_id, "root-1");
        assert!(tree.nodes.is_empty());
    }

    #[test]
    fn test_expand_branch() {
        let mut engine = ToTEngine::new("root-1".to_string());
        engine.init("Solve this problem".to_string());
        engine.expand_branch(
            "root-1",
            "node-1".to_string(),
            "Try approach A".to_string(),
            None,
            None,
        );
        assert_eq!(engine.tree.nodes.len(), 2);
        let node = engine.tree.get("node-1").unwrap();
        assert_eq!(node.parent_id.as_deref(), Some("root-1"));
        assert_eq!(node.depth, 1);
        assert_eq!(node.status, NodeStatus::Active);
    }

    #[test]
    fn test_prune_keeps_top_branches() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root thought".to_string());

        for i in 0..5 {
            engine.expand_branch(
                "root",
                format!("node-{}", i),
                format!("branch {}", i),
                None,
                None,
            );
        }

        assert_eq!(engine.tree.active_nodes_at_depth(1).len(), 5);

        engine.tree.beam_width = 2;
        let pruned = engine.score_and_prune();

        // 5 total - 2 kept = 3 pruned
        assert_eq!(pruned.len(), 3);
        assert_eq!(engine.tree.active_nodes_at_depth(1).len(), 2);
    }

    #[test]
    fn test_best_path_update() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root".to_string());
        engine.expand_branch("root", "a".to_string(), "branch A".to_string(), None, None);
        engine.expand_branch("root", "b".to_string(), "branch B".to_string(), None, None);

        // Give root and b the same high score, a a lower score.
        // b is always best because it has a strictly higher score than a,
        // regardless of HashMap iteration order.
        engine.complete_node("root", 0.9);
        engine.complete_node("b", 0.9);
        engine.complete_node("a", 0.8);
        engine.tree.update_best_path();
        assert_eq!(engine.tree.best_branch(), vec!["root", "b"]);
        assert_eq!(engine.tree.best_score(), 0.9);

        // Give a a higher score - best path should now be [root, a]
        engine.complete_node("a", 0.95);
        engine.tree.update_best_path();
        assert_eq!(engine.tree.best_branch(), vec!["root", "a"]);
    }

    #[test]
    fn test_tot_trigger_user_keyword() {
        let trigger = ToTEngine::should_trigger_tot("help me explore the alternatives", false);
        assert!(trigger.user_keyword);
        assert!(trigger.should_trigger());
    }

    #[test]
    fn test_tot_trigger_no_trigger() {
        let trigger = ToTEngine::should_trigger_tot("Just say hello", false);
        assert!(!trigger.user_keyword);
        assert!(!trigger.should_trigger());
    }

    #[test]
    fn test_tot_trigger_repeated_failure() {
        let trigger = ToTEngine::should_trigger_tot("read the file", true);
        assert!(trigger.repeated_failure);
        assert!(trigger.should_trigger());
    }

    #[test]
    fn test_stats() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root".to_string());
        engine.expand_branch("root", "n1".to_string(), "t1".into(), None, None);
        engine.expand_branch("root", "n2".to_string(), "t2".into(), None, None);
        engine.complete_node("n1", 0.7);
        engine.complete_node("n2", 0.8);

        let stats = engine.stats();
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.completed_nodes, 2);
        assert_eq!(stats.best_score, 0.8);
    }

    #[test]
    fn test_branch_path() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root".to_string());
        engine.expand_branch("root", "a".to_string(), "A".into(), None, None);
        engine.expand_branch("a", "a1".to_string(), "A1".into(), None, None);

        let node = engine.tree.get("a1").unwrap();
        let path = node.branch_path(&engine.tree);
        let ids: Vec<_> = path.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, ["root", "a", "a1"]);
    }

    #[test]
    fn test_frontend_json() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("start".to_string());
        engine.expand_branch("root", "b1".to_string(), "branch 1".into(), None, None);
        engine.complete_node("b1", 0.8);

        let json = engine.to_frontend_json();
        assert_eq!(json["rootId"], "root");
        assert!(json["stats"]["completedNodes"].is_number());
    }

    #[test]
    fn test_should_continue() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root".to_string());
        engine.expand_branch("root", "n1".to_string(), "t1".into(), None, None);
        // max_depth = 10, depth 1 is not terminal
        assert!(engine.should_continue());
    }
}
