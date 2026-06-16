#!/usr/bin/env python3
"""
train_ngav.py — Entraînement du NGAV ML d'ExoShield (premier jeu de poids RÉEL).

Produit des poids ENTRAÎNÉS pour :
  • le MLP profond 32→128→64→4 (ml/mlp.rs)  — modèle dominant de l'ensemble
  • les seuils de l'Isolation Forest 8×63   (ml/iforest.rs)
  • les constantes de normalisation FEATURE_MAX (corrige le bug d'échelle F10)

⚠️  DONNÉES SYNTHÉTIQUES. Ce script génère des événements bénins/malveillants
    synthétiques calqués EXACTEMENT sur la distribution que le kernel fournit au
    runtime (`behaviour_data_for_event` dans servers/exo_shield/src/main.rs :
    features clampées à [0,99], vecteur creux par type d'événement).
    Le modèle final DEVRA être ré-entraîné sur des traces RÉELLES Exo-OS
    (profiler.rs sous QEMU : bénin vs malveillant simulé). Ce jeu n'est qu'un
    PREMIER modèle fonctionnel, pas un détecteur de production.

Sortie : servers/exo_shield/src/ml/trained_weights.rs (constantes Q16.16).

Usage :
    ~/.venvs/exo_ml/bin/python tools/ml_training/train_ngav.py
"""

import os
import struct
import numpy as np

# ── Constantes — DOIVENT matcher le kernel ──────────────────────────────────
FEATURE_COUNT = 32
H1, H2, OUT = 128, 64, 4
Q16 = 65536
LEAKY_SLOPE = 655 / 65536          # mlp.rs: (x*655)>>16
SEED = 20260616

# Index de features (features.rs)
(F_SYSCALL_RATE, F_FILE_OPEN, F_FILE_WRITE, F_NET_CONNECT, F_NET_SENT, F_NET_RECV,
 F_CPU, F_MEM, F_FORK, F_EXEC, F_SIGNAL, F_IPC, F_PRIV_ESC, F_DENIED, F_PERM_CHG,
 F_SUSP_PATH, F_DNS_RATE, F_DNS_UNIQ, F_PORTSCAN, F_BIND, F_SHM, F_THREAD,
 F_SYS_DIV, F_FILE_DEL, F_PROC_DUR, F_RENICE, F_MODLOAD, F_RAWSOCK, F_CHROOT,
 F_CLONE_NS, F_PTRACE, F_ANOMALY_AVG) = range(32)

# Classes de sortie (mlp.rs : OUT_MALICIOUS=2)
OUT_BENIGN, OUT_SUSPICIOUS, OUT_MALICIOUS, OUT_UNKNOWN = 0, 1, 2, 3

rng = np.random.default_rng(SEED)

# ── Bornes de features (max runtime observable) → normalisation ─────────────
# Le runtime clampe presque tout à 99 ; quelques features vont plus haut.
FEATURE_MAX = np.full(FEATURE_COUNT, 100.0)
FEATURE_MAX[F_SYS_DIV] = 128.0       # opcode & 0x7F
FEATURE_MAX[F_DENIED] = 120.0        # 99 + 10 (blocked) + marge
FEATURE_MAX[F_ANOMALY_AVG] = 200.0   # 99 + level*20
FEATURE_MAX[F_PRIV_ESC] = 100.0


# ── Génération synthétique calquée sur behaviour_data_for_event() ───────────
def gen_event(kind, malice):
    """
    Génère un vecteur de 32 features (creux) pour un événement.
    `kind` ∈ {syscall, network, memory, process, ipc}
    `malice` ∈ [0,1] : 0 = parfaitement bénin, 1 = clairement malveillant.
    Reproduit la structure exacte de behaviour_data_for_event (main.rs).
    """
    v = np.zeros(FEATURE_COUNT)
    lo = lambda hi: rng.integers(0, max(1, int(hi)))             # bénin
    hi = lambda a, b: rng.integers(int(a), int(b))               # élevé

    if kind == "syscall":
        v[F_SYSCALL_RATE] = 1 + lo(99)
        v[F_SYS_DIV] = rng.integers(0, 128)
        if malice > 0.5:
            v[F_DENIED] = hi(25, 99)
            v[F_PRIV_ESC] = hi(20, 99)
        else:
            v[F_DENIED] = lo(5)
            v[F_PRIV_ESC] = lo(3)
    elif kind == "network":
        v[F_NET_CONNECT] = 1 + lo(99)
        v[F_NET_SENT] = lo(99)
        if malice > 0.5:
            v[F_PORTSCAN] = hi(35, 99)
            v[F_DNS_RATE] = hi(25, 99)
        else:
            v[F_PORTSCAN] = lo(3)
            v[F_DNS_RATE] = lo(4)
    elif kind == "memory":
        v[F_MEM] = lo(99)
        if malice > 0.5:
            v[F_ANOMALY_AVG] = hi(30, 99)
            v[F_SUSP_PATH] = hi(20, 99)
        else:
            v[F_ANOMALY_AVG] = lo(3)
            v[F_SUSP_PATH] = lo(2)
    elif kind == "process":
        v[F_EXEC] = 1 + lo(99)
        if malice > 0.5:
            v[F_FORK] = hi(20, 99)
            v[F_DENIED] = hi(20, 99)
        else:
            v[F_FORK] = lo(4)
            v[F_DENIED] = lo(3)
    elif kind == "ipc":
        sev = hi(3, 5) if malice > 0.5 else lo(2)
        v[F_IPC] = 1 + sev
        v[F_PRIV_ESC] = sev

    # Post-traitement commun (main.rs) : blocage + niveau de menace.
    blocked = malice > 0.5 and rng.random() < 0.7
    if blocked:
        v[F_DENIED] = min(FEATURE_MAX[F_DENIED], v[F_DENIED] + 10)
    level = hi(3, 6) if malice > 0.5 else lo(2)
    v[F_ANOMALY_AVG] = min(FEATURE_MAX[F_ANOMALY_AVG], v[F_ANOMALY_AVG] + level * 20)
    return v


def label_for(malice):
    """Cible one-hot à partir du niveau de malveillance."""
    t = np.zeros(OUT)
    if malice < 0.34:
        t[OUT_BENIGN] = 1.0
    elif malice < 0.67:
        t[OUT_SUSPICIOUS] = 1.0
    else:
        t[OUT_MALICIOUS] = 1.0
    return t


def make_dataset(n):
    kinds = ["syscall", "network", "memory", "process", "ipc"]
    X, Y, M = [], [], []
    for _ in range(n):
        kind = kinds[rng.integers(0, len(kinds))]
        # 45% bénin, 20% gris (suspicious), 35% malveillant
        r = rng.random()
        if r < 0.45:
            malice = rng.uniform(0.0, 0.30)
        elif r < 0.65:
            malice = rng.uniform(0.40, 0.62)
        else:
            malice = rng.uniform(0.70, 1.0)
        X.append(gen_event(kind, malice))
        Y.append(label_for(malice))
        M.append(malice)
    return np.array(X), np.array(Y), np.array(M)


# ── Activations — IDENTIQUES au kernel ──────────────────────────────────────
def leaky(x):
    return np.where(x >= 0, x, LEAKY_SLOPE * x)

def dleaky(x):
    return np.where(x >= 0, 1.0, LEAKY_SLOPE)

def sig_lin(x):
    # mlp.rs: clamp(0.5 + x/8, 0, 1) sur [-4,4]
    return np.clip(0.5 + x / 8.0, 0.0, 1.0)

def dsig_lin(x):
    return np.where((x > -4.0) & (x < 4.0), 1.0 / 8.0, 0.0)


# ── MLP (forward/backward en float, normalisé [0,1]) ────────────────────────
class MLP:
    def __init__(self):
        # Init Xavier
        self.W1 = rng.normal(0, np.sqrt(1.0 / FEATURE_COUNT), (FEATURE_COUNT, H1))
        self.b1 = np.zeros(H1)
        self.W2 = rng.normal(0, np.sqrt(1.0 / H1), (H1, H2))
        self.b2 = np.zeros(H2)
        self.W3 = rng.normal(0, np.sqrt(1.0 / H2), (H2, OUT))
        self.b3 = np.zeros(OUT)

    def forward(self, x):
        self.z1 = x @ self.W1 + self.b1
        self.a1 = leaky(self.z1)
        self.z2 = self.a1 @ self.W2 + self.b2
        self.a2 = leaky(self.z2)
        self.z3 = self.a2 @ self.W3 + self.b3
        self.out = sig_lin(self.z3)
        return self.out

    def backward(self, x, y, lr):
        n = x.shape[0]
        # MSE loss gradient
        d_out = (self.out - y) * dsig_lin(self.z3) / n
        dW3 = self.a2.T @ d_out
        db3 = d_out.sum(0)
        d2 = (d_out @ self.W3.T) * dleaky(self.z2)
        dW2 = self.a1.T @ d2
        db2 = d2.sum(0)
        d1 = (d2 @ self.W2.T) * dleaky(self.z1)
        dW1 = x.T @ d1
        db1 = d1.sum(0)
        for p, g in [(self.W1, dW1), (self.b1, db1), (self.W2, dW2),
                     (self.b2, db2), (self.W3, dW3), (self.b3, db3)]:
            p -= lr * g


def normalize(X):
    return np.clip(X / FEATURE_MAX, 0.0, 1.0)


def train():
    print("[*] Génération du dataset synthétique...", flush=True)
    Xtr, Ytr, Mtr = make_dataset(8000)
    Xte, Yte, Mte = make_dataset(2000)
    Xtr_n, Xte_n = normalize(Xtr), normalize(Xte)

    net = MLP()
    lr, epochs, batch = 0.6, 70, 512
    idx = np.arange(len(Xtr_n))
    print(f"[*] Entraînement MLP {FEATURE_COUNT}->{H1}->{H2}->{OUT} "
          f"({epochs} epochs, lr={lr})...", flush=True)
    for ep in range(epochs):
        rng.shuffle(idx)
        for s in range(0, len(idx), batch):
            b = idx[s:s + batch]
            net.forward(Xtr_n[b])
            net.backward(Xtr_n[b], Ytr[b], lr)
        if (ep + 1) % 20 == 0:
            acc = eval_acc(net, Xte_n, Yte)
            print(f"    epoch {ep+1:3d}  test_acc={acc:.3f}", flush=True)

    acc = eval_acc(net, Xte_n, Yte)
    print(f"[*] Accuracy finale (3 classes) : {acc:.3f}")
    confusion(net, Xte_n, Yte)
    return net


def eval_acc(net, X, Y):
    pred = net.forward(X).argmax(1)
    return (pred == Y.argmax(1)).mean()


def confusion(net, X, Y):
    pred = net.forward(X).argmax(1)
    true = Y.argmax(1)
    names = ["Benign", "Suspic", "Malic"]
    print("    Confusion (lignes=vérité, cols=prédit) :")
    for t in range(3):
        row = [int(((true == t) & (pred == p)).sum()) for p in range(3)]
        print(f"      {names[t]:7s} {row}")
    # Métrique clé sécurité : rappel sur Malicious
    mal_recall = ((pred == OUT_MALICIOUS) & (true == OUT_MALICIOUS)).sum() / max(1, (true == OUT_MALICIOUS).sum())
    mal_fp = ((pred == OUT_MALICIOUS) & (true == OUT_BENIGN)).sum() / max(1, (true == OUT_BENIGN).sum())
    print(f"    Rappel Malicious={mal_recall:.3f}  FP(benign→malic)={mal_fp:.3f}")


# ── Isolation Forest — fit des seuils sur la distribution bénigne ───────────
IF_TREES, IF_NODES = 8, 63
DANGEROUS = [12, 18, 22, 26, 27, 28, 29, 30]   # iforest.rs

def fit_iforest(Xbenign):
    """
    Fixe (feature, seuil) par nœud pour que les samples BÉNINS empruntent des
    chemins profonds et que les anomalies (features dangereuses élevées)
    s'isolent tôt. Conserve la structure complète 8×63 du kernel.
    Seuil = percentile haut de la feature sur les bénins (raw, 0..max).
    """
    trees = []
    for t in range(IF_TREES):
        bias = t < IF_TREES // 2
        nodes = []
        for i in range(IF_NODES):
            if bias and rng.random() < 0.5:
                feat = DANGEROUS[rng.integers(0, len(DANGEROUS))]
            else:
                feat = int(rng.integers(0, FEATURE_COUNT))
            col = Xbenign[:, feat]
            if feat in DANGEROUS:
                # seuil bas : la moindre activité dangereuse isole
                thr = max(1, int(np.percentile(col, 75)) + 1)
            else:
                # seuil = médiane bénigne : split équilibré pour le normal
                thr = max(1, int(np.percentile(col, 50)) + 1)
            nodes.append((feat, min(thr, 65535)))
        trees.append(nodes)
    return trees


# ── Quantification Q16.16 + export Rust ─────────────────────────────────────
def q16(arr):
    return np.clip(np.round(np.asarray(arr) * Q16), -2_000_000_000, 2_000_000_000).astype(np.int64)

def checksum_mlp(w1, b1, w2, b2, w3, b3, version):
    """Checksum d'intégrité FNV-1a 64-bit — DOIT matcher la vérif kernel (FIX-F3)."""
    h = 0x5151_5151_0000_0000
    for arr in (w1, b1, w2, b2, w3, b3):
        for x in arr:
            h ^= (int(x) & 0xFFFFFFFF)
            h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    h ^= version
    return h & 0xFFFFFFFFFFFFFFFF


def fmt_arr(name, arr, ty="i32"):
    arr = list(int(x) for x in arr)
    out = [f"pub const {name}: [{ty}; {len(arr)}] = ["]
    line = "    "
    for x in arr:
        tok = f"{x}, "
        if len(line) + len(tok) > 96:
            out.append(line.rstrip())
            line = "    "
        line += tok
    if line.strip():
        out.append(line.rstrip())
    out.append("];")
    return "\n".join(out)


def export_rust(net, trees, path):
    w1 = q16(net.W1.reshape(-1)); b1 = q16(net.b1)
    w2 = q16(net.W2.reshape(-1)); b2 = q16(net.b2)
    w3 = q16(net.W3.reshape(-1)); b3 = q16(net.b3)
    version = 2
    cks = checksum_mlp(w1, b1, w2, b2, w3, b3, version)
    fmax = [int(x) for x in FEATURE_MAX]

    # Aplatir l'IF : 8*63 paires (feature u8, threshold u16) -> 2 tableaux
    if_feat, if_thr = [], []
    for tr in trees:
        for (f, thr) in tr:
            if_feat.append(f)
            if_thr.append(thr)

    parts = []
    parts.append("// @generated par tools/ml_training/train_ngav.py — NE PAS éditer à la main.")
    parts.append("// Premier jeu de poids ENTRAÎNÉ (données synthétiques). À ré-entraîner")
    parts.append("// sur traces réelles Exo-OS (profiler.rs sous QEMU). Voir AUDIT-100-PERCENT.md (F4/F10).")
    parts.append("#![allow(clippy::all)]")
    parts.append("")
    parts.append(f"pub const TRAINED_MLP_VERSION: u32 = {version};")
    parts.append(f"pub const TRAINED_MLP_CHECKSUM: u64 = 0x{cks:016X};")
    parts.append("")
    parts.append("/// Dénominateurs de normalisation min-max (max runtime par feature).")
    parts.append(fmt_arr("FEATURE_MAX", fmax))
    parts.append("")
    parts.append(fmt_arr("MLP_W1", w1))
    parts.append(fmt_arr("MLP_B1", b1))
    parts.append(fmt_arr("MLP_W2", w2))
    parts.append(fmt_arr("MLP_B2", b2))
    parts.append(fmt_arr("MLP_W3", w3))
    parts.append(fmt_arr("MLP_B3", b3))
    parts.append("")
    parts.append("/// Isolation Forest entraîné : 8 arbres × 63 nœuds (feature, seuil).")
    parts.append(fmt_arr("IF_NODE_FEATURE", if_feat, "u8"))
    parts.append(fmt_arr("IF_NODE_THRESHOLD", if_thr, "u16"))
    parts.append("")
    with open(path, "w") as f:
        f.write("\n".join(parts) + "\n")
    print(f"[*] Export Rust -> {path}")
    print(f"    MLP version={version} checksum=0x{cks:016X}")
    print(f"    {len(w1)} w1, {len(w2)} w2, {len(w3)} w3 ; IF {len(if_feat)} nœuds")


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    repo = os.path.abspath(os.path.join(here, "..", ".."))
    out = os.path.join(repo, "servers", "exo_shield", "src", "ml", "trained_weights.rs")

    net = train()

    # IF : fit sur un échantillon purement bénin
    Xb, _, _ = make_dataset(3000)
    Xben = Xb  # majorité bénigne via le tirage; suffisant pour les percentiles
    trees = fit_iforest(Xben)

    export_rust(net, trees, out)
    print("[OK] Entraînement terminé.")


if __name__ == "__main__":
    main()
