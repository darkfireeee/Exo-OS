// kernel/src/fs/exofs/gc/tricolor.rs
//
// ==============================================================================
// Algorithme tri-colore Blanc/Gris/Noir pour ExoFS GC
// Ring 0 . no_std . Exo-OS
//
// L'algorithme tri-colore est l'invariant central du GC :
//   Blanc  : objet non atteint depuis les racines (candidat a la collecte)
//   Gris   : atteint mais dont les enfants n'ont pas encore ete explores
//   Noir   : atteint ET tous ses enfants explores (vivant definitif)
//
// Conformite :
//   GC-02 : les Relations sont traversees (cycles orphelins sinon non collectes)
//   GC-03 : file grise bornee a MAX_GC_GREY_QUEUE = 1 000 000
//   GC-04 : try_reserve() obligatoire pour la file grise
//   RECUR-01 : iteratif uniquement — pile explicite heap-allouee
//   REFCNT-01: ref_count check avant tout acces objet
// ==============================================================================

#![allow(dead_code)]

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ==============================================================================
// Constantes
// ==============================================================================

/// Borne superieure de la file grise (GC-03).
pub const MAX_GC_GREY_QUEUE: usize = 1_000_000;

/// Taille initiale allouee pour la file grise (compromis memoire/realloc).
pub const GC_GREY_QUEUE_INITIAL_CAPACITY: usize = 4096;

/// Taille initiale de la BlobIndex.
pub const GC_BLOB_INDEX_INITIAL_CAPACITY: usize = 8192;

/// Taille du batch de marquage par iteration (evite de tenir le lock trop longtemps).
pub const GC_MARK_BATCH_SIZE: usize = 256;

// ==============================================================================
// TriColor — couleur d'un noeud
// ==============================================================================

/// Couleur d'un noeud dans l'algorithme tri-colore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TriColor {
    /// Non atteint. Candidat a la collecte si encore Blanc a la fin.
    White = 0,
    /// Atteint mais enfants non encore explores. Dans la file grise.
    Grey  = 1,
    /// Atteint et tous ses enfants explores. Definitivement vivant.
    Black = 2,
}

impl TriColor {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => TriColor::White,
            1 => TriColor::Grey,
            2 => TriColor::Black,
            _ => TriColor::White,
        }
    }

    pub fn is_live(self) -> bool {
        !matches!(self, TriColor::White)
    }

    pub fn name(self) -> &'static str {
        match self {
            TriColor::White => "White",
            TriColor::Grey  => "Grey",
            TriColor::Black => "Black",
        }
    }
}

impl fmt::Display for TriColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ==============================================================================
// BlobNode — noeud du graphe GC
// ==============================================================================

/// Representation d'un P-Blob dans le graphe tri-colore.
#[derive(Debug, Clone)]
pub struct BlobNode {
    /// Identifiant unique du blob.
    pub id:           BlobId,
    /// Couleur courante dans l'algorithme.
    pub color:        TriColor,
    /// Taille physique en octets (pour bytes_freed).
    pub phys_size:    u64,
    /// Epoch de creation du blob.
    pub create_epoch: u64,
    /// Epoch du dernier acces connu.
    pub last_epoch:   u64,
    /// Compteur de references au debut de la passe.
    pub ref_count:    u32,
    /// true si ce blob est epingle (GC-07 : EPOCH_PINNED).
    pub pinned:       bool,
}

impl BlobNode {
    /// Cree un nouveau noeud avec couleur initiale Blanc.
    pub fn new(
        id:           BlobId,
        phys_size:    u64,
        create_epoch: u64,
        last_epoch:   u64,
        ref_count:    u32,
        pinned:       bool,
    ) -> Self {
        Self {
            id,
            color: TriColor::White,
            phys_size,
            create_epoch,
            last_epoch,
            ref_count,
            pinned,
        }
    }

    /// Un blob est collectible si :
    ///   - couleur Blanc a la fin du marquage, ET
    ///   - ref_count == 0, ET
    ///   - non epingle (GC-07)
    pub fn is_collectible(&self) -> bool {
        self.color == TriColor::White
            && self.ref_count == 0
            && !self.pinned
    }

    /// Passe ce noeud en Gris (premiere atteinte).
    pub fn mark_grey(&mut self) {
        if self.color == TriColor::White {
            self.color = TriColor::Grey;
        }
    }

    /// Passe ce noeud en Noir (tous enfants explores).
    pub fn mark_black(&mut self) {
        self.color = TriColor::Black;
    }
}

// ==============================================================================
// TricolorWorkspace — espace de travail complet pour une passe GC
// ==============================================================================

/// Espace de travail tri-colore pour une passe GC complete.
///
/// Contient :
///   - L'index de tous les blobs connus (BlobId → BlobNode)
///   - La file grise bornee a MAX_GC_GREY_QUEUE (GC-03)
///   - Les statistiques de marquage
pub struct TricolorWorkspace {
    /// Index principal : BlobId → BlobNode.
    pub nodes: BTreeMap<BlobId, BlobNode>,
    /// File grise (FIFO). Bornee a MAX_GC_GREY_QUEUE.
    grey_queue: VecDeque<BlobId>,
    /// Statistiques de la phase de marquage.
    pub mark_stats: MarkStats,
}

impl TricolorWorkspace {
    /// Cree un espace de travail vide avec pre-allocation.
    ///
    /// OOM-02 : try_reserve avant toute insertion.
    pub fn new() -> ExofsResult<Self> {
        let mut grey_queue = VecDeque::new();
        grey_queue
            .try_reserve_exact(GC_GREY_QUEUE_INITIAL_CAPACITY)
            .map_err(|_| ExofsError::NoMemory)?;

        Ok(Self {
            nodes:      BTreeMap::new(),
            grey_queue,
            mark_stats: MarkStats::default(),
        })
    }

    /// Insere un noeud dans l'index.
    ///
    /// OOM-02 : pas de try_reserve sur BTreeMap (pas supporte),
    /// l'erreur d'allocation sera propagee par le kernel allocator.
    pub fn insert_node(&mut self, node: BlobNode) {
        self.nodes.insert(node.id, node);
    }

    /// Retourne l'epoch de création d'un blob par son BlobId.
    pub fn node_epoch(&self, id: &BlobId) -> Option<u64> {
        self.nodes.get(id).map(|n| n.create_epoch)
    }

    /// Combien de blobs sont dans l'index.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Longueur courante de la file grise.
    pub fn grey_queue_len(&self) -> usize {
        self.grey_queue.len()
    }

    // ── Grisement ────────────────────────────────────────────────────────────

    /// Grise un blob s'il est Blanc.
    ///
    /// GC-03 : retourne `ExofsError::GcQueueFull` si la file est pleine.
    /// GC-04 : try_reserve avant push.
    pub fn grey(&mut self, id: BlobId) -> ExofsResult<()> {
        // Ne rien faire si le blob n'est pas dans l'index.
        let node = match self.nodes.get_mut(&id) {
            Some(n) => n,
            None    => return Ok(()),
        };

        if node.color != TriColor::White {
            // Deja Gris ou Noir — pas besoin de re-griser.
            return Ok(());
        }

        // GC-03 : file bornee.
        if self.grey_queue.len() >= MAX_GC_GREY_QUEUE {
            return Err(ExofsError::GcQueueFull);
        }

        // GC-04 : try_reserve avant push.
        self.grey_queue
            .try_reserve_exact(1)
            .map_err(|_| ExofsError::NoMemory)?;

        node.mark_grey();
        self.grey_queue.push_back(id);
        self.mark_stats.greyed = self.mark_stats.greyed.saturating_add(1);
        Ok(())
    }

    /// Grise plusieurs blobs depuis une slice de racines.
    ///
    /// GC-06 : les racines proviennent des EpochRoots slots A/B/C.
    pub fn grey_roots(&mut self, roots: &[BlobId]) -> ExofsResult<()> {
        for &id in roots {
            self.grey(id)?;
        }
        self.mark_stats.roots_greyed =
            self.mark_stats.roots_greyed.saturating_add(roots.len() as u64);
        Ok(())
    }

    // ── Marquage ─────────────────────────────────────────────────────────────

    /// Retire le prochain Gris de la file.
    pub fn pop_grey(&mut self) -> Option<BlobId> {
        self.grey_queue.pop_front()
    }

    /// Marque un blob Noir (tous ses enfants explores).
    ///
    /// Retourne `true` si le blob existait et est maintenant Noir.
    pub fn blacken(&mut self, id: &BlobId) -> bool {
        match self.nodes.get_mut(id) {
            Some(n) if n.color == TriColor::Grey => {
                n.mark_black();
                self.mark_stats.marked_black =
                    self.mark_stats.marked_black.saturating_add(1);
                true
            }
            _ => false,
        }
    }

    /// Couleur courante d'un blob (None si inconnu).
    pub fn color_of(&self, id: &BlobId) -> Option<TriColor> {
        self.nodes.get(id).map(|n| n.color)
    }

    // ── Balayage ─────────────────────────────────────────────────────────────

    /// Collecte tous les blobs Blancs restants (candidats a la suppression).
    ///
    /// Un blob Blanc est collectible seulement si :
    ///   - ref_count == 0 (REFCNT-01)
    ///   - non epingle (GC-07)
    ///
    /// Retourne la liste des blobs a supprimer avec leurs tailles.
    pub fn collect_white(&self) -> Vec<(BlobId, u64)> {
        let mut result = Vec::new();
        for node in self.nodes.values() {
            if node.is_collectible() {
                result.push((node.id, node.phys_size));
            }
        }
        result
    }

    /// Compte les blobs par couleur.
    pub fn color_counts(&self) -> (usize, usize, usize) {
        let mut white = 0usize;
        let mut grey  = 0usize;
        let mut black = 0usize;
        for node in self.nodes.values() {
            match node.color {
                TriColor::White => white += 1,
                TriColor::Grey  => grey  += 1,
                TriColor::Black => black += 1,
            }
        }
        (white, grey, black)
    }

    /// Verifie l'invariant tri-colore : pas de noir→blanc direct.
    ///
    /// Appele en mode debug pour detecter les violations.
    /// Un blob Noir ne peut pas avoir d'enfant Blanc dans le graphe.
    /// (Cette verification necessite le walker — appelee par le marker.)
    pub fn validate_no_black_to_white(&self, _edges: &[(BlobId, BlobId)]) -> bool {
        // Verification : pour chaque arete (src, dst), si src est Noir alors dst ne peut etre Blanc.
        for (src, dst) in _edges {
            let src_color = self.color_of(src).unwrap_or(TriColor::White);
            let dst_color = self.color_of(dst).unwrap_or(TriColor::White);
            if src_color == TriColor::Black && dst_color == TriColor::White {
                return false;
            }
        }
        true
    }
}

// ==============================================================================
// MarkStats — statistiques de la phase de marquage
// ==============================================================================

/// Statistiques collectees durant la phase de marquage.
#[derive(Debug, Default, Clone)]
pub struct MarkStats {
    /// Blobs grises depuis les racines.
    pub roots_greyed:     u64,
    /// Total des blobs grises (racines + propagation).
    pub greyed:           u64,
    /// Blobs marques Noirs.
    pub marked_black:     u64,
    /// Blobs traverses par le marker.
    pub traversed:        u64,
    /// Nombre de fois que la file grise a deborde (GC-03 : Err retourne).
    pub queue_overflows:  u64,
    /// Edges de relation traverses (GC-02).
    pub relation_edges:   u64,
    /// Iterations du marquage.
    pub mark_iterations:  u64,
}

impl fmt::Display for MarkStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MarkStats[roots={} greyed={} black={} traversed={} overflows={} \
             rel_edges={} iters={}]",
            self.roots_greyed,
            self.greyed,
            self.marked_black,
            self.traversed,
            self.queue_overflows,
            self.relation_edges,
            self.mark_iterations,
        )
    }
}

// ==============================================================================
// SweepResult — resultat de la phase de balayage
// ==============================================================================

/// Resultat de la phase de balayage.
#[derive(Debug, Default, Clone)]
pub struct SweepResult {
    /// Blobs effectivement collectes (etaient Blancs avec ref_count=0).
    pub blobs_swept:    u64,
    /// Octets liberes.
    pub bytes_freed:    u64,
    /// Blobs Blancs mais non collectes (EPOCH_PINNED ou ref_count > 0).
    pub blobs_skipped:  u64,
    /// Blobs ajoutes a la DeferredDeleteQueue.
    pub deferred_count: u64,
}

impl fmt::Display for SweepResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SweepResult[swept={} freed={}B skipped={} deferred={}]",
            self.blobs_swept,
            self.bytes_freed,
            self.blobs_skipped,
            self.deferred_count,
        )
    }
}

// ==============================================================================
// Utilitaires
// ==============================================================================

/// Verifie que la file grise n'est pas pleine avant une insertion.
///
/// GC-03 : utilise dans le marker pour reporter si pleine.
pub fn grey_queue_has_room(workspace: &TricolorWorkspace) -> bool {
    workspace.grey_queue_len() < MAX_GC_GREY_QUEUE
}

/// Calcule le ratio de remplissage de la file grise en pourcent.
pub fn grey_queue_fill_ratio_x100(workspace: &TricolorWorkspace) -> u64 {
    let len = workspace.grey_queue_len() as u64;
    let max = MAX_GC_GREY_QUEUE as u64;
    if max == 0 { return 100; }
    len.saturating_mul(100) / max
}

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob_id(b: u8) -> BlobId {
        let mut arr = [0u8; 32];
        arr[0] = b;
        BlobId(arr)
    }

    fn make_node(id: BlobId) -> BlobNode {
        BlobNode::new(id, 4096, 1, 2, 0, false)
    }

    #[test]
    fn test_tricolor_new_and_insert() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(1);
        ws.insert_node(make_node(id));
        assert_eq!(ws.len(), 1);
        assert_eq!(ws.color_of(&id), Some(TriColor::White));
    }

    #[test]
    fn test_grey_and_blacken() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(2);
        ws.insert_node(make_node(id));

        ws.grey(id).unwrap();
        assert_eq!(ws.color_of(&id), Some(TriColor::Grey));
        assert_eq!(ws.grey_queue_len(), 1);

        let popped = ws.pop_grey();
        assert_eq!(popped, Some(id));

        ws.blacken(&id);
        assert_eq!(ws.color_of(&id), Some(TriColor::Black));
    }

    #[test]
    fn test_grey_already_grey_ignored() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(3);
        ws.insert_node(make_node(id));
        ws.grey(id).unwrap();
        ws.grey(id).unwrap(); // doublon
        assert_eq!(ws.grey_queue_len(), 1); // pas double
    }

    #[test]
    fn test_collect_white_with_ref_zero() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(4);
        ws.insert_node(BlobNode::new(id, 1024, 1, 2, 0, false));
        let whites = ws.collect_white();
        assert_eq!(whites.len(), 1);
        assert_eq!(whites[0].1, 1024);
    }

    #[test]
    fn test_collect_white_skips_pinned() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(5);
        ws.insert_node(BlobNode::new(id, 1024, 1, 2, 0, true)); // pinned=true
        let whites = ws.collect_white();
        assert!(whites.is_empty());
    }

    #[test]
    fn test_collect_white_skips_nonzero_ref() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id = make_blob_id(6);
        ws.insert_node(BlobNode::new(id, 1024, 1, 2, 1, false)); // ref_count=1
        let whites = ws.collect_white();
        assert!(whites.is_empty());
    }

    #[test]
    fn test_color_counts() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let id1 = make_blob_id(7);
        let id2 = make_blob_id(8);
        let id3 = make_blob_id(9);
        ws.insert_node(make_node(id1));
        ws.insert_node(make_node(id2));
        ws.insert_node(make_node(id3));
        ws.grey(id1).unwrap();
        ws.grey(id2).unwrap();
        ws.blacken(&id1);
        let (w, g, b) = ws.color_counts();
        assert_eq!(w, 1);
        assert_eq!(g, 1);
        assert_eq!(b, 1);
    }

    #[test]
    fn test_grey_roots() {
        let mut ws = TricolorWorkspace::new().unwrap();
        let ids: Vec<BlobId> = (10..15).map(make_blob_id).collect();
        for &id in &ids {
            ws.insert_node(make_node(id));
        }
        ws.grey_roots(&ids).unwrap();
        assert_eq!(ws.grey_queue_len(), 5);
        assert_eq!(ws.mark_stats.roots_greyed, 5);
    }

    #[test]
    fn test_tricolor_display() {
        assert_eq!(TriColor::White.to_string(), "White");
        assert_eq!(TriColor::Grey.to_string(),  "Grey");
        assert_eq!(TriColor::Black.to_string(), "Black");
    }

    #[test]
    fn test_mark_stats_display() {
        let s = MarkStats { greyed: 42, marked_black: 30, ..Default::default() };
        let d = alloc::format!("{}", s);
        assert!(d.contains("greyed=42"));
    }
}
