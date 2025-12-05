//! Audio processing graph

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Node ID
pub type NodeId = u64;

/// Port ID (node_id, port_index)
pub type PortId = (NodeId, u32);

/// Audio processing node type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    /// Audio source (device input)
    Source,
    /// Audio sink (device output)
    Sink,
    /// Processing filter
    Filter,
    /// Stream (application)
    Stream,
}

/// Audio processing node
pub struct AudioNode {
    /// Node ID
    pub id: NodeId,
    /// Node name
    pub name: String,
    /// Node type
    pub node_type: NodeType,
    /// Input ports
    pub inputs: Vec<u32>,
    /// Output ports
    pub outputs: Vec<u32>,
}

/// Connection between nodes
#[derive(Debug, Clone)]
pub struct Connection {
    /// Source port
    pub source: PortId,
    /// Destination port
    pub destination: PortId,
}

/// Audio processing graph
pub struct AudioGraph {
    /// Nodes in the graph
    nodes: BTreeMap<NodeId, AudioNode>,
    /// Connections between ports
    connections: Vec<Connection>,
    /// Next node ID
    next_id: NodeId,
}

impl AudioGraph {
    /// Create new audio graph
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            connections: Vec::new(),
            next_id: 1,
        }
    }

    /// Add node to graph
    pub fn add_node(&mut self, name: String, node_type: NodeType) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;

        let node = AudioNode {
            id,
            name,
            node_type,
            inputs: Vec::new(),
            outputs: Vec::new(),
        };

        self.nodes.insert(id, node);
        id
    }

    /// Remove node from graph
    pub fn remove_node(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.connections
            .retain(|c| c.source.0 != id && c.destination.0 != id);
    }

    /// Connect two ports
    pub fn connect(&mut self, source: PortId, destination: PortId) -> bool {
        // Validate ports exist
        if !self.nodes.contains_key(&source.0) || !self.nodes.contains_key(&destination.0) {
            return false;
        }

        self.connections.push(Connection {
            source,
            destination,
        });
        true
    }

    /// Disconnect ports
    pub fn disconnect(&mut self, source: PortId, destination: PortId) {
        self.connections.retain(|c| c.source != source || c.destination != destination);
    }

    /// Get node by ID
    pub fn get_node(&self, id: NodeId) -> Option<&AudioNode> {
        self.nodes.get(&id)
    }

    /// Get all nodes
    pub fn nodes(&self) -> impl Iterator<Item = &AudioNode> {
        self.nodes.values()
    }

    /// Get connections for a node
    pub fn get_connections(&self, node_id: NodeId) -> Vec<&Connection> {
        self.connections
            .iter()
            .filter(|c| c.source.0 == node_id || c.destination.0 == node_id)
            .collect()
    }
}

impl Default for AudioGraph {
    fn default() -> Self {
        Self::new()
    }
}
