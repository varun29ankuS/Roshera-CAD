// src/merkle.rs

//! Merkle Tree Implementation for .ros Chunk Integrity
//!
//! Provides cryptographic proof of chunk integrity with:
//! - Efficient inclusion proofs
//! - Batch verification
//! - Tree serialization
//! - Multiple hash algorithm support

use crate::ros_fs::util::to_hex;
use crate::ros_fs::{Result, RosFileError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::fmt;

/// Hash type for Merkle tree nodes
pub type MerkleHash = [u8; 32];

/// Extended hash for SHA-512
pub type MerkleHash512 = [u8; 64];

/// Hash algorithms supported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlgorithm {
    Sha256,
    Sha512,
}

impl HashAlgorithm {
    pub fn hash_size(&self) -> usize {
        match self {
            HashAlgorithm::Sha256 => 32,
            HashAlgorithm::Sha512 => 64,
        }
    }
}

/// A node in the Merkle tree
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleNode {
    pub hash: Vec<u8>,
    pub left: Option<Box<MerkleNode>>,
    pub right: Option<Box<MerkleNode>>,
    pub is_leaf: bool,
    pub index: Option<usize>, // Leaf index for leaf nodes
}

impl MerkleNode {
    /// Create a leaf node
    pub fn leaf(data: &[u8], index: usize, algorithm: HashAlgorithm) -> Self {
        let hash = match algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"leaf:");
                hasher.update(data);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"leaf:");
                hasher.update(data);
                hasher.finalize().to_vec()
            }
        };

        MerkleNode {
            hash,
            left: None,
            right: None,
            is_leaf: true,
            index: Some(index),
        }
    }

    /// Create an internal node
    pub fn internal(left: MerkleNode, right: MerkleNode, algorithm: HashAlgorithm) -> Self {
        let hash = match algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"node:");
                hasher.update(&left.hash);
                hasher.update(&right.hash);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"node:");
                hasher.update(&left.hash);
                hasher.update(&right.hash);
                hasher.finalize().to_vec()
            }
        };

        MerkleNode {
            hash,
            left: Some(Box::new(left)),
            right: Some(Box::new(right)),
            is_leaf: false,
            index: None,
        }
    }

    /// Get hash as hex string
    pub fn hash_hex(&self) -> String {
        to_hex(&self.hash)
    }
}

/// Merkle tree structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleTree {
    pub root: Option<MerkleNode>,
    pub leaves: Vec<Vec<u8>>,
    pub algorithm: HashAlgorithm,
}

impl MerkleTree {
    /// Create an empty tree
    pub fn new(algorithm: HashAlgorithm) -> Self {
        MerkleTree {
            root: None,
            leaves: Vec::new(),
            algorithm,
        }
    }

    /// Build tree from leaf data
    pub fn from_leaves(leaves: Vec<Vec<u8>>, algorithm: HashAlgorithm) -> Result<Self> {
        if leaves.is_empty() {
            return Ok(MerkleTree::new(algorithm));
        }

        // Create leaf nodes
        let mut nodes: Vec<MerkleNode> = leaves
            .iter()
            .enumerate()
            .map(|(i, data)| MerkleNode::leaf(data, i, algorithm))
            .collect();

        // Build tree bottom-up
        while nodes.len() > 1 {
            let mut next_level = Vec::new();

            for chunk in nodes.chunks(2) {
                let node = if chunk.len() == 2 {
                    MerkleNode::internal(chunk[0].clone(), chunk[1].clone(), algorithm)
                } else {
                    // Odd number - duplicate last node
                    MerkleNode::internal(chunk[0].clone(), chunk[0].clone(), algorithm)
                };
                next_level.push(node);
            }

            nodes = next_level;
        }

        Ok(MerkleTree {
            root: nodes.into_iter().next(),
            leaves: leaves.clone(),
            algorithm,
        })
    }

    /// Get the root hash
    pub fn root_hash(&self) -> Option<&[u8]> {
        self.root.as_ref().map(|r| r.hash.as_slice())
    }

    /// Get root hash as hex
    pub fn root_hash_hex(&self) -> Option<String> {
        self.root.as_ref().map(|r| r.hash_hex())
    }

    /// Generate inclusion proof for a leaf
    pub fn generate_proof(&self, leaf_index: usize) -> Result<MerkleProof> {
        if leaf_index >= self.leaves.len() {
            return Err(RosFileError::Other {
                message: format!("Leaf index {} out of bounds", leaf_index),
                source: None,
            });
        }

        let root = self.root.as_ref().ok_or_else(|| RosFileError::Other {
            message: "Empty tree has no proofs".to_string(),
            source: None,
        })?;

        let leaf_data = &self.leaves[leaf_index];
        let leaf_hash = match self.algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"leaf:");
                hasher.update(leaf_data);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"leaf:");
                hasher.update(leaf_data);
                hasher.finalize().to_vec()
            }
        };

        let mut proof_path = Vec::new();
        self.build_proof_path(root, leaf_index, &mut proof_path)?;

        Ok(MerkleProof {
            leaf_hash,
            leaf_index,
            siblings: proof_path,
            algorithm: self.algorithm,
        })
    }

    /// Build proof path recursively
    fn build_proof_path(
        &self,
        node: &MerkleNode,
        target_index: usize,
        path: &mut Vec<ProofNode>,
    ) -> Result<bool> {
        if node.is_leaf {
            return Ok(node.index == Some(target_index));
        }

        let left = node.left.as_ref().ok_or_else(|| RosFileError::Other {
            message: "Invalid tree structure".to_string(),
            source: None,
        })?;

        let right = node.right.as_ref().ok_or_else(|| RosFileError::Other {
            message: "Invalid tree structure".to_string(),
            source: None,
        })?;

        // Try left subtree
        if self.build_proof_path(left, target_index, path)? {
            path.push(ProofNode {
                hash: right.hash.clone(),
                position: Position::Right,
            });
            return Ok(true);
        }

        // Try right subtree
        if self.build_proof_path(right, target_index, path)? {
            path.push(ProofNode {
                hash: left.hash.clone(),
                position: Position::Left,
            });
            return Ok(true);
        }

        Ok(false)
    }

    /// Verify the tree is correctly constructed
    pub fn verify(&self) -> bool {
        match &self.root {
            None => self.leaves.is_empty(),
            Some(root) => self.verify_node(root),
        }
    }

    fn verify_node(&self, node: &MerkleNode) -> bool {
        if node.is_leaf {
            return true;
        }

        let (left, right) = match (&node.left, &node.right) {
            (Some(l), Some(r)) => (l, r),
            _ => return false,
        };

        // Verify hash
        let computed_hash = match self.algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"node:");
                hasher.update(&left.hash);
                hasher.update(&right.hash);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"node:");
                hasher.update(&left.hash);
                hasher.update(&right.hash);
                hasher.finalize().to_vec()
            }
        };

        if computed_hash != node.hash {
            return false;
        }

        self.verify_node(left) && self.verify_node(right)
    }

    /// Get tree depth
    pub fn depth(&self) -> usize {
        match &self.root {
            None => 0,
            Some(root) => self.node_depth(root),
        }
    }

    fn node_depth(&self, node: &MerkleNode) -> usize {
        if node.is_leaf {
            1
        } else {
            let left_depth = node.left.as_ref().map(|n| self.node_depth(n)).unwrap_or(0);
            let right_depth = node.right.as_ref().map(|n| self.node_depth(n)).unwrap_or(0);
            1 + left_depth.max(right_depth)
        }
    }
}

/// Position of sibling in proof
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    Left,
    Right,
}

/// Node in a Merkle proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofNode {
    pub hash: Vec<u8>,
    pub position: Position,
}

/// Merkle inclusion proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub leaf_hash: Vec<u8>,
    pub leaf_index: usize,
    pub siblings: Vec<ProofNode>,
    pub algorithm: HashAlgorithm,
}

impl MerkleProof {
    /// Verify this proof against a root hash
    pub fn verify(&self, root_hash: &[u8], leaf_data: &[u8]) -> bool {
        // Verify leaf hash
        let computed_leaf_hash = match self.algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"leaf:");
                hasher.update(leaf_data);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"leaf:");
                hasher.update(leaf_data);
                hasher.finalize().to_vec()
            }
        };

        if computed_leaf_hash != self.leaf_hash {
            return false;
        }

        // Compute root from proof
        let mut current_hash = self.leaf_hash.clone();

        for sibling in &self.siblings {
            current_hash = match sibling.position {
                Position::Left => self.hash_pair(&sibling.hash, &current_hash),
                Position::Right => self.hash_pair(&current_hash, &sibling.hash),
            };
        }

        current_hash == root_hash
    }

    fn hash_pair(&self, left: &[u8], right: &[u8]) -> Vec<u8> {
        match self.algorithm {
            HashAlgorithm::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(b"node:");
                hasher.update(left);
                hasher.update(right);
                hasher.finalize().to_vec()
            }
            HashAlgorithm::Sha512 => {
                let mut hasher = Sha512::new();
                hasher.update(b"node:");
                hasher.update(left);
                hasher.update(right);
                hasher.finalize().to_vec()
            }
        }
    }

    /// Get proof size in bytes
    pub fn size_bytes(&self) -> usize {
        self.leaf_hash.len()
            + self
                .siblings
                .iter()
                .map(|s| s.hash.len() + 1)
                .sum::<usize>()
    }
}

/// Batch Merkle proof for multiple leaves
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMerkleProof {
    pub proofs: Vec<MerkleProof>,
    pub algorithm: HashAlgorithm,
}

impl BatchMerkleProof {
    /// Create a batch proof
    pub fn new(proofs: Vec<MerkleProof>, algorithm: HashAlgorithm) -> Self {
        BatchMerkleProof { proofs, algorithm }
    }

    /// Verify all proofs in the batch
    pub fn verify_all(&self, root_hash: &[u8], leaves: &[Vec<u8>]) -> bool {
        if self.proofs.len() != leaves.len() {
            return false;
        }

        self.proofs
            .iter()
            .zip(leaves.iter())
            .all(|(proof, leaf_data)| proof.verify(root_hash, leaf_data))
    }

    /// Get total size of batch proof
    pub fn size_bytes(&self) -> usize {
        self.proofs.iter().map(|p| p.size_bytes()).sum()
    }
}

/// Helper to compute simple Merkle root (backwards compatible)
pub fn compute_merkle_root(hashes: Vec<MerkleHash>) -> MerkleHash {
    if hashes.is_empty() {
        return [0u8; 32];
    }

    let tree = MerkleTree::from_leaves(
        hashes.into_iter().map(|h| h.to_vec()).collect(),
        HashAlgorithm::Sha256,
    )
    .unwrap();

    let root = tree.root_hash().unwrap();
    let mut result = [0u8; 32];
    result.copy_from_slice(root);
    result
}

/// Tree visualization helper
impl fmt::Display for MerkleTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.root {
            None => write!(f, "Empty Merkle Tree"),
            Some(root) => {
                writeln!(f, "Merkle Tree ({:?}):", self.algorithm)?;
                self.fmt_node(f, root, "", true)
            }
        }
    }
}

impl MerkleTree {
    fn fmt_node(
        &self,
        f: &mut fmt::Formatter<'_>,
        node: &MerkleNode,
        prefix: &str,
        is_last: bool,
    ) -> fmt::Result {
        let connector = if is_last { "└── " } else { "├── " };
        let hash_str = format!(
            "{}...{}",
            &node.hash_hex()[..8],
            &node.hash_hex()[node.hash_hex().len() - 8..]
        );

        if node.is_leaf {
            writeln!(
                f,
                "{}{}{} (leaf {})",
                prefix,
                connector,
                hash_str,
                node.index.unwrap()
            )?;
        } else {
            writeln!(f, "{}{}{}", prefix, connector, hash_str)?;

            let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

            if let Some(left) = &node.left {
                self.fmt_node(f, left, &new_prefix, false)?;
            }

            if let Some(right) = &node.right {
                self.fmt_node(f, right, &new_prefix, true)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree_construction() {
        let leaves = vec![
            b"chunk1".to_vec(),
            b"chunk2".to_vec(),
            b"chunk3".to_vec(),
            b"chunk4".to_vec(),
        ];

        let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).unwrap();
        assert!(tree.root.is_some());
        assert_eq!(tree.depth(), 3); // log2(4) + 1
        assert!(tree.verify());
    }

    #[test]
    fn test_inclusion_proof() {
        let leaves = vec![
            b"data1".to_vec(),
            b"data2".to_vec(),
            b"data3".to_vec(),
            b"data4".to_vec(),
        ];

        let tree = MerkleTree::from_leaves(leaves.clone(), HashAlgorithm::Sha256).unwrap();
        let root_hash = tree.root_hash().unwrap();

        // Generate and verify proof for each leaf
        for (i, leaf) in leaves.iter().enumerate() {
            let proof = tree.generate_proof(i).unwrap();
            assert!(proof.verify(root_hash, leaf));

            // Verify proof fails with wrong data
            assert!(!proof.verify(root_hash, b"wrong data"));
        }
    }

    #[test]
    fn test_odd_number_of_leaves() {
        let leaves = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];

        let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).unwrap();
        assert!(tree.verify());

        // Check that last leaf was duplicated correctly
        let proof = tree.generate_proof(2).unwrap();
        assert!(proof.verify(tree.root_hash().unwrap(), b"c"));
    }

    #[test]
    fn test_sha512_algorithm() {
        let leaves = vec![b"test1".to_vec(), b"test2".to_vec()];

        let tree256 = MerkleTree::from_leaves(leaves.clone(), HashAlgorithm::Sha256).unwrap();
        let tree512 = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha512).unwrap();

        // Different algorithms should produce different roots
        assert_ne!(
            tree256.root_hash().unwrap().len(),
            tree512.root_hash().unwrap().len()
        );
        assert_eq!(tree256.root_hash().unwrap().len(), 32);
        assert_eq!(tree512.root_hash().unwrap().len(), 64);
    }

    #[test]
    fn test_batch_proof() {
        let leaves = vec![
            b"leaf1".to_vec(),
            b"leaf2".to_vec(),
            b"leaf3".to_vec(),
            b"leaf4".to_vec(),
        ];

        let tree = MerkleTree::from_leaves(leaves.clone(), HashAlgorithm::Sha256).unwrap();
        let root_hash = tree.root_hash().unwrap();

        // Create batch proof for leaves 0 and 2
        let proof0 = tree.generate_proof(0).unwrap();
        let proof2 = tree.generate_proof(2).unwrap();

        let batch = BatchMerkleProof::new(vec![proof0, proof2], HashAlgorithm::Sha256);

        // Verify batch
        let batch_leaves = vec![leaves[0].clone(), leaves[2].clone()];
        assert!(batch.verify_all(root_hash, &batch_leaves));

        // Wrong order should fail
        let wrong_order = vec![leaves[2].clone(), leaves[0].clone()];
        assert!(!batch.verify_all(root_hash, &wrong_order));
    }

    #[test]
    fn test_tree_display() {
        let leaves = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()];

        let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).unwrap();
        let display = format!("{}", tree);

        assert!(display.contains("Merkle Tree"));
        assert!(display.contains("leaf 0"));
        assert!(display.contains("leaf 3"));
    }

    #[test]
    fn test_empty_tree() {
        let tree = MerkleTree::from_leaves(vec![], HashAlgorithm::Sha256).unwrap();
        assert!(tree.root.is_none());
        assert_eq!(tree.depth(), 0);
        assert!(tree.verify());
    }

    #[test]
    fn test_single_leaf() {
        let leaves = vec![b"single".to_vec()];
        let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).unwrap();

        assert!(tree.root.is_some());
        assert_eq!(tree.depth(), 1);
        assert!(tree.verify());

        let proof = tree.generate_proof(0).unwrap();
        assert_eq!(proof.siblings.len(), 0); // No siblings for single leaf
    }
}
