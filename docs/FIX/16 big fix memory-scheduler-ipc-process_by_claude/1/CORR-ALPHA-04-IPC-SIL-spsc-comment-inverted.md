# CORR-ALPHA-04 — IPC : Commentaire algorithme SPSC inversé (head ↔ tail)

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🟠 SIL — Inversion sémantique silencieuse  
> **Fichier :** `kernel/src/ipc/ring/spsc.rs`  
> **Struct :** `SpscRing`  
> **Sévérité :** Majeure — toute implémentation d'un consommateur basée sur ce commentaire sera structurellement inversée

---

## 1. Description du bug

### 1.1 Le commentaire d'algorithme

Au sommet de `spsc.rs`, le commentaire décrit l'algorithme comme :

```rust
// ALGORITHME :
//   • Producteur : lit tail, accède cell[tail % N], écrit données, avance tail.
//   • Consommateur : lit head, accède cell[head % N], lit données, avance head.
```

### 1.2 La documentation des champs

```rust
/// Position du prochain slot à écrire (cache line 0).
head: CachePad,
/// Position du prochain slot à lire (cache line 1).
tail: CachePad,
```

### 1.3 Le code réel

```rust
pub fn push_copy(&self, src: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    // ...
    let pos = self.head.0.load(Ordering::Relaxed);  // ← producteur lit HEAD
    let cell = self.cell_at(pos);
    // ...
    self.head.0.store(pos + 1, Ordering::Release);   // ← producteur avance HEAD
}
```

### 1.4 La contradiction

| Source | Pointeur producteur | Pointeur consommateur |
|--------|--------------------|-----------------------|
| **Commentaire algorithme** | `tail` | `head` |
| **Documentation des champs** | `head` (write pos) | `tail` (read pos) |
| **Code réel** | `head` | `tail` |

Le commentaire d'algorithme **dit l'inverse de la réalité**. Il affirme que le producteur utilise `tail` et le consommateur utilise `head`, alors que c'est l'opposé dans le code et dans la doc des champs.

### 1.5 Impact potentiel

Si un développeur implémente le consommateur (`pop_into`) en suivant le commentaire d'algorithme plutôt que la doc des champs, il lira `self.head` (le pointeur d'écriture) au lieu de `self.tail` (le pointeur de lecture), produisant :
- Des lectures de slots non encore écrits (données indéfinies)
- Des races entre producteur et consommateur sur `head`
- Un ring qui ne progresse jamais correctement

Ce type d'inversion est particulièrement insidieux car le ring peut sembler fonctionner lors des tests simples (producer et consumer sur le même CPU, séquentiels) mais rompre sous charge SMP.

---

## 2. Correctif

### Fichier : `kernel/src/ipc/ring/spsc.rs`

**Avant :**
```rust
// ALGORITHME :
//   • Producteur : lit tail, accède cell[tail % N], écrit données, avance tail.
//   • Consommateur : lit head, accède cell[head % N], lit données, avance head.
```

**Après :**
```rust
// ALGORITHME :
//   • Producteur : lit head (index d'écriture), accède cell[head % N],
//     écrit données + séquence, avance head (Release).
//   • Consommateur : lit tail (index de lecture), accède cell[tail % N],
//     attend que la séquence du slot corresponde, lit données, avance tail (Release).
//   • head = pointeur d'écriture exclusif du producteur (cache line 0).
//   • tail = pointeur de lecture exclusif du consommateur (cache line 1).
//   • Séparation sur cache lines distinctes : élimine le false sharing producteur/consommateur.
```

### Clarification optionnelle des champs (même fichier)

```rust
/// Index du prochain slot à ÉCRIRE — propriété exclusive du PRODUCTEUR (cache line 0).
/// Lu par le producteur en Relaxed (seul écrivain), publié en Release après écriture.
head: CachePad,
/// Index du prochain slot à LIRE — propriété exclusive du CONSOMMATEUR (cache line 1).
/// Lu par le consommateur en Relaxed (seul lecteur), publié en Release après lecture.
tail: CachePad,
```

---

## 3. Vérification du code existant

Le code de `push_copy()` et le code de `pop_into()` (non montré dans l'audit mais inférable) **sont corrects** — seul le commentaire est inversé. Pour confirmation :

```rust
// push_copy — producteur utilise head : ✅ CORRECT
let pos = self.head.0.load(Ordering::Relaxed);
// ...
self.head.0.store(pos + 1, Ordering::Release);

// pop_into — consommateur devrait utiliser tail : À VÉRIFIER
// Si la fonction existe et utilise self.tail, le code est correct.
// Si elle utilise self.head (erreur suivant le commentaire), c'est un bug fonctionnel.
```

**Action requise :** vérifier que `pop_into()` / `pop()` utilise `self.tail.0` et non `self.head.0`.

---

## 4. Conformité IPC-01

La règle IPC-01 spécifie :
```rust
pub struct SpscRing {
    head: CachePad,  // ligne de cache producteur
    tail: CachePad,  // ligne de cache consommateur (séparée)
```

La correction du commentaire aligne désormais la description algorithmique avec la convention `head=write, tail=read` de la règle IPC-01 et du code réel.

---

## 5. Impact scope

- **Fichier modifié :** `kernel/src/ipc/ring/spsc.rs`
- **Nature :** correction de commentaire uniquement (+ recommandation de vérification de `pop_into`)
- **Aucun changement de comportement runtime** pour le code existant
- **Documentation externe :** `docs/kernel/ipc/ring_buffers.md` — vérifier que la description du SPSC est cohérente avec head=write/tail=read

---

*— claude-alpha*
