//! # Raft — foundation for distributed consensus
//!
//! When the `raft` feature is enabled, the server can run as a Raft node. WAL-style
//! operations are proposed to the cluster and only confirmed after replication to a
//! majority. This module exposes cluster state (nodes, leader, replication status)
//! for the Cluster API and dashboard.

use serde::{Deserialize, Serialize};

/// Cluster node info for API and dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNodeInfo {
    pub id: String,
    pub addr: String,
    /// "leader" | "follower" | "learner" | "replica"
    pub role: String,
    /// Replication lag (last log index applied). Only meaningful for followers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replication_lag: Option<u64>,
}

/// Cluster status returned by GET /api/v1/cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStatus {
    /// Whether Raft consensus is enabled (feature "raft" and configured).
    pub raft_enabled: bool,
    /// Current leader node id (e.g. "1"). None if no leader yet or single node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_id: Option<String>,
    /// All known nodes (including this one).
    pub nodes: Vec<ClusterNodeInfo>,
}

/// Returns cluster status for standalone mode (no Raft). Single node, role from replica_of.
pub fn cluster_status_standalone(this_addr: &str, is_replica_of: bool) -> ClusterStatus {
    let role = if is_replica_of { "replica" } else { "leader" };
    ClusterStatus {
        raft_enabled: false,
        leader_id: Some("1".to_string()),
        nodes: vec![ClusterNodeInfo {
            id: "1".to_string(),
            addr: this_addr.to_string(),
            role: role.to_string(),
            replication_lag: None,
        }],
    }
}

/// Handle to the Raft node when feature "raft" is enabled.
/// Holds Raft runtime and provides cluster status and propose path.
#[cfg(feature = "raft")]
pub struct RaftHandle {
    pub node_id: u64,
    pub raft: std::sync::Arc<openraft::Raft<FerresRaftTypeConfig>>,
}

#[cfg(feature = "raft")]
use std::io::Cursor;

#[cfg(feature = "raft")]
openraft::declare_raft_types!(
    pub FerresRaftTypeConfig:
        D = WalReplicateRequest,
        R = (),
);

/// Application request: one serialized WAL payload (collection + ops) to replicate.
#[cfg(feature = "raft")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalReplicateRequest {
    pub collection: String,
    /// Serialized WAL operations (e.g. bincode of Vec<WalOp>).
    pub payload: Vec<u8>,
}

#[cfg(feature = "raft")]
impl RaftHandle {
    pub fn current_status(&self, this_addr: &str) -> ClusterStatus {
        let metrics_guard = self.raft.metrics();
        let metrics = metrics_guard.borrow();
        let leader_id = metrics.current_leader.map(|id| format!("{}", id));
        let this_node_id = self.node_id;
        let this_role = if metrics.current_leader == Some(this_node_id) {
            "leader"
        } else {
            "follower"
        };
        let mut nodes = vec![ClusterNodeInfo {
            id: self.node_id.to_string(),
            addr: this_addr.to_string(),
            role: this_role.to_string(),
            replication_lag: None,
        }];
        let membership = &metrics.membership_config;
        for (node_id, _node) in membership.nodes() {
            let id_str = format!("{}", node_id);
            if id_str != nodes[0].id {
                nodes.push(ClusterNodeInfo {
                    id: id_str,
                    addr: format!("node-{}", node_id),
                    role: "follower".to_string(),
                    replication_lag: None,
                });
            }
        }
        ClusterStatus {
            raft_enabled: true,
            leader_id,
            nodes,
        }
    }

    /// Proposes a WAL payload to the cluster and waits until it is committed (replicated to majority).
    /// Returns Ok(()) when the entry is committed; the state machine will apply it on all nodes.
    pub async fn replicate_then_confirm(
        &self,
        request: WalReplicateRequest,
    ) -> Result<
        (),
        openraft::error::RaftError<
            u64,
            openraft::error::ClientWriteError<u64, openraft::impls::BasicNode>,
        >,
    > {
        self.raft.client_write(request).await.map(|_| ())
    }
}

/// Stub when feature "raft" is not enabled.
#[cfg(not(feature = "raft"))]
pub struct RaftHandle;
