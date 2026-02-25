// kernel/src/security/crypto/x25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// X25519 — ECDH sur Curve25519 (Montgomery form)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Courbe : Montgomery y² = x³ + 486662x² + x  sur GF(2²⁵⁵ - 19)
//   • Scalaire : 255 bits, clamp systématique (RFC 7748)
//   • Algorithme : Montgomery Ladder (time-constant, clamped)
//   • Représentation : radix 2²⁵·⁵ — 10 limbs u32 (référence : libsodium)
//
// RÈGLE X25519-01 : Le scalaire est TOUJOURS clampé selon RFC 7748.
// RÈGLE X25519-02 : Le point de base est TOUJOURS u (= 9).
// RÈGLE X25519-03 : Vérification low-order point via compare_ct().
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::compiler_fence;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X25519Error {
    /// Point reçu de faible ordre (attaque).
    LowOrderPoint,
    /// Paramètre invalide.
    InvalidInput,
}

// ─────────────────────────────────────────────────────────────────────────────
// Arithmétique GF(p) pour p = 2^255 - 19
// Représentation : 10 limbs u32 en radix 2^25.5
// limbs[0,2,4,6,8] : puissance 25 bits
// limbs[1,3,5,7,9] : puissance 26 bits
// ─────────────────────────────────────────────────────────────────────────────

/// Élément de champ GF(2^255-19) en représentation radix-2^25.5 sur 10 limbs i64.
#[derive(Clone, Copy)]
struct Fe([i64; 10]);

impl Fe {
    const ZERO: Fe = Fe([0; 10]);
    const ONE:  Fe = Fe([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    /// Point de base u = 9
    fn base_u() -> Fe {
        Fe([9, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    }

    /// Décode depuis 32 bytes little-endian (RFC 7748).
    fn from_bytes(bytes: &[u8; 32]) -> Fe {
        let mut b = *bytes;
        // Masquer le bit de signe
        b[31] &= 0x7F;

        let mut h = [0i64; 10];
        // Décoder 10 limbs depuis little-endian
        let load4 = |b: &[u8], i: usize| -> i64 {
            (b[i] as i64)
                | ((b[i+1] as i64) << 8)
                | ((b[i+2] as i64) << 16)
                | ((b[i+3] as i64) << 24)
        };
        let load3 = |b: &[u8], i: usize| -> i64 {
            (b[i] as i64) | ((b[i+1] as i64) << 8) | ((b[i+2] as i64) << 16)
        };

        h[0] = load4(&b,  0)        & 0x3FFFFFF;
        h[1] = (load3(&b, 3) >> 2)  & 0x1FFFFFF;
        h[2] = (load4(&b, 4) >> 3)  & 0x3FFFFFF;
        h[3] = (load4(&b, 7) >> 5)  & 0x1FFFFFF;
        h[4] = (load4(&b, 9) >> 6)  & 0x3FFFFFF;
        h[5] = load4(&b, 12)         & 0x1FFFFFF;
        h[6] = (load4(&b, 13) >> 1)  & 0x3FFFFFF;
        h[7] = (load4(&b, 16) >> 3)  & 0x1FFFFFF;
        h[8] = (load4(&b, 18) >> 4)  & 0x3FFFFFF;
        h[9] = (load4(&b, 21) >> 6)  & 0x1FFFFFF;
        Fe(h)
    }

    /// Encode vers 32 bytes little-endian.
    fn to_bytes(self) -> [u8; 32] {
        let s = self.reduce_final();
        let h = s.0;
        let mut out = [0u8; 32];
        out[0]  = (h[0]        ) as u8;
        out[1]  = (h[0]  >>  8 ) as u8;
        out[2]  = (h[0]  >> 16 ) as u8;
        out[3]  = ((h[0] >> 24) | (h[1] << 2)) as u8;
        out[4]  = (h[1]  >>  6 ) as u8;
        out[5]  = (h[1]  >> 14 ) as u8;
        out[6]  = ((h[1] >> 22) | (h[2] << 3)) as u8;
        out[7]  = (h[2]  >>  5 ) as u8;
        out[8]  = (h[2]  >> 13 ) as u8;
        out[9]  = ((h[2] >> 21) | (h[3] << 5)) as u8;
        out[10] = (h[3]  >>  3 ) as u8;
        out[11] = (h[3]  >> 11 ) as u8;
        out[12] = ((h[3] >> 19) | (h[4] << 6)) as u8;
        out[13] = (h[4]  >>  2 ) as u8;
        out[14] = (h[4]  >> 10 ) as u8;
        out[15] = (h[4]  >> 18 ) as u8;
        out[16] = (h[5]        ) as u8;
        out[17] = (h[5]  >>  8 ) as u8;
        out[18] = (h[5]  >> 16 ) as u8;
        out[19] = ((h[5] >> 24) | (h[6] << 1)) as u8;
        out[20] = (h[6]  >>  7 ) as u8;
        out[21] = (h[6]  >> 15 ) as u8;
        out[22] = ((h[6] >> 23) | (h[7] << 3)) as u8;
        out[23] = (h[7]  >>  5 ) as u8;
        out[24] = (h[7]  >> 13 ) as u8;
        out[25] = ((h[7] >> 21) | (h[8] << 4)) as u8;
        out[26] = (h[8]  >>  4 ) as u8;
        out[27] = (h[8]  >> 12 ) as u8;
        out[28] = ((h[8] >> 20) | (h[9] << 6)) as u8;
        out[29] = (h[9]  >>  2 ) as u8;
        out[30] = (h[9]  >> 10 ) as u8;
        out[31] = (h[9]  >> 18 ) as u8;
        out
    }

    /// Réduction finale pour s'assurer h < p.
    fn reduce_final(self) -> Fe {
        let h = self.0;
        let mut q = (19 * h[9] + (1 << 24)) >> 25;
        q = (h[0] + q) >> 26;
        q = (h[1] + q) >> 25;
        q = (h[2] + q) >> 26;
        q = (h[3] + q) >> 25;
        q = (h[4] + q) >> 26;
        q = (h[5] + q) >> 25;
        q = (h[6] + q) >> 26;
        q = (h[7] + q) >> 25;
        q = (h[8] + q) >> 26;
        q = (h[9] + q) >> 25;
        let mut r = [0i64; 10];
        r[0] = h[0] + 19 * q;
        r[1] = h[1] + (r[0] >> 26); r[0] &= 0x3FFFFFF;
        r[2] = h[2] + (r[1] >> 25); r[1] &= 0x1FFFFFF;
        r[3] = h[3] + (r[2] >> 26); r[2] &= 0x3FFFFFF;
        r[4] = h[4] + (r[3] >> 25); r[3] &= 0x1FFFFFF;
        r[5] = h[5] + (r[4] >> 26); r[4] &= 0x3FFFFFF;
        r[6] = h[6] + (r[5] >> 25); r[5] &= 0x1FFFFFF;
        r[7] = h[7] + (r[6] >> 26); r[6] &= 0x3FFFFFF;
        r[8] = h[8] + (r[7] >> 25); r[7] &= 0x1FFFFFF;
        r[9] = h[9] + (r[8] >> 26); r[8] &= 0x3FFFFFF;
                                     r[9] &= 0x1FFFFFF;
        Fe(r)
    }

    fn add(self, rhs: Fe) -> Fe {
        let mut r = [0i64; 10];
        for i in 0..10 { r[i] = self.0[i] + rhs.0[i]; }
        Fe(r)
    }

    fn sub(self, rhs: Fe) -> Fe {
        let mut r = [0i64; 10];
        for i in 0..10 { r[i] = self.0[i] - rhs.0[i]; }
        Fe(r)
    }

    /// Multiplication mod p — algorithme schoolbook sur les limbs.
    fn mul(self, rhs: Fe) -> Fe {
        let f = self.0;
        let g = rhs.0;
        let f1_19 = 19 * f[1]; let f2_19 = 19 * f[2]; let f3_19 = 19 * f[3];
        let f4_19 = 19 * f[4]; let f5_19 = 19 * f[5]; let f6_19 = 19 * f[6];
        let f7_19 = 19 * f[7]; let f8_19 = 19 * f[8]; let f9_19 = 19 * f[9];
        let f0g0 = f[0]*g[0]; let f0g1 = f[0]*g[1]; let f0g2 = f[0]*g[2];
        let f0g3 = f[0]*g[3]; let f0g4 = f[0]*g[4]; let f0g5 = f[0]*g[5];
        let f0g6 = f[0]*g[6]; let f0g7 = f[0]*g[7]; let f0g8 = f[0]*g[8];
        let f0g9 = f[0]*g[9];
        let f1g0   = f[1]*g[0];   let f1g1_2  = f[1]*2*g[1]; let f1g2   = f[1]*g[2];
        let f1g3_2 = f[1]*2*g[3]; let f1g4   = f[1]*g[4];    let f1g5_2 = f[1]*2*g[5];
        let f1g6   = f[1]*g[6];   let f1g7_2  = f[1]*2*g[7]; let f1g8   = f[1]*g[8];
        let f1g9_38= f9_19*2*g[1];
        let f2g0   = f[2]*g[0]; let f2g1   = f[2]*g[1]; let f2g2   = f[2]*g[2];
        let f2g3   = f[2]*g[3]; let f2g4   = f[2]*g[4]; let f2g5   = f[2]*g[5];
        let f2g6   = f[2]*g[6]; let f2g7   = f[2]*g[7]; let f2g8_19= f8_19*g[2];
        let f2g9_19= f9_19*g[2];
        let f3g0   = f[3]*g[0]; let f3g1_2  = f[3]*2*g[1]; let f3g2   = f[3]*g[2];
        let f3g3_2 = f[3]*2*g[3]; let f3g4   = f[3]*g[4];  let f3g5_2 = f[3]*2*g[5];
        let f3g6   = f[3]*g[6];   let f3g7_38= f7_19*2*g[3]; let f3g8_19 = f8_19*g[3];
        let f3g9_38= f9_19*2*g[3];
        let f4g0   = f[4]*g[0]; let f4g1   = f[4]*g[1]; let f4g2   = f[4]*g[2];
        let f4g3   = f[4]*g[3]; let f4g4   = f[4]*g[4]; let f4g5   = f[4]*g[5];
        let f4g6_19= f6_19*g[4]; let f4g7_19= f7_19*g[4]; let f4g8_19= f8_19*g[4];
        let f4g9_19= f9_19*g[4];
        let f5g0   = f[5]*g[0]; let f5g1_2  = f[5]*2*g[1]; let f5g2   = f[5]*g[2];
        let f5g3_2 = f[5]*2*g[3]; let f5g4   = f[5]*g[4];  let f5g5_38= f5_19*2*g[5];
        let f5g6_19= f6_19*g[5]; let f5g7_38= f7_19*2*g[5]; let f5g8_19= f8_19*g[5];
        let f5g9_38= f9_19*2*g[5];
        let f6g0   = f[6]*g[0]; let f6g1   = f[6]*g[1]; let f6g2   = f[6]*g[2];
        let f6g3   = f[6]*g[3]; let f6g4_19= f6_19*g[4]; let f6g5_19= f6_19*g[5];
        let f6g6_19= f6_19*g[6]; let f6g7_19= f7_19*g[6]; let f6g8_19= f8_19*g[6];
        let f6g9_19= f9_19*g[6];
        let f7g0   = f[7]*g[0]; let f7g1_2  = f[7]*2*g[1]; let f7g2   = f[7]*g[2];
        let f7g3_38= f7_19*2*g[3]; let f7g4_19= f7_19*g[4]; let f7g5_38= f7_19*2*g[5];
        let f7g6_19= f7_19*g[6]; let f7g7_38= f7_19*2*g[7]; let f7g8_19= f8_19*g[7];
        let f7g9_38= f9_19*2*g[7];
        let f8g0   = f[8]*g[0]; let f8g1   = f[8]*g[1]; let f8g2_19 = f8_19*g[2];
        let f8g3_19= f8_19*g[3]; let f8g4_19= f8_19*g[4]; let f8g5_19= f8_19*g[5];
        let f8g6_19= f8_19*g[6]; let f8g7_19= f8_19*g[7]; let f8g8_19= f8_19*g[8];
        let f8g9_19= f9_19*g[8];
        let f9g0   = f[9]*g[0]; let f9g1_38= f9_19*2*g[1]; let f9g2_19= f9_19*g[2];
        let f9g3_38= f9_19*2*g[3]; let f9g4_19= f9_19*g[4]; let f9g5_38= f9_19*2*g[5];
        let f9g6_19= f9_19*g[6]; let f9g7_38= f9_19*2*g[7]; let f9g8_19= f9_19*g[8];
        let f9g9_38= f9_19*2*g[9];

        let mut h = [0i64; 10];
        h[0] = f0g0+f1g9_38+f2g8_19+f3g7_38+f4g6_19+f5g5_38+f6g4_19+f7g3_38+f8g2_19+f9g1_38;
        h[1] = f0g1+f1g0+f2g9_19+f3g8_19+f4g7_19+f5g6_19+f6g5_19+f7g4_19+f8g3_19+f9g2_19;
        h[2] = f0g2+f1g1_2+f2g0+f3g9_38+f4g8_19+f5g7_38+f6g6_19+f7g5_38+f8g4_19+f9g3_38;
        h[3] = f0g3+f1g2+f2g1+f3g0+f4g9_19+f5g8_19+f6g7_19+f7g6_19+f8g5_19+f9g4_19;
        h[4] = f0g4+f1g3_2+f2g2+f3g1_2+f4g0+f5g9_38+f6g8_19+f7g7_38+f8g6_19+f9g5_38;
        h[5] = f0g5+f1g4+f2g3+f3g2+f4g1+f5g0+f6g9_19+f7g8_19+f8g7_19+f9g6_19;
        h[6] = f0g6+f1g5_2+f2g4+f3g3_2+f4g2+f5g1_2+f6g0+f7g9_38+f8g8_19+f9g7_38;
        h[7] = f0g7+f1g6+f2g5+f3g4+f4g3+f5g2+f6g1+f7g0+f8g9_19+f9g8_19;
        h[8] = f0g8+f1g7_2+f2g6+f3g5_2+f4g4+f5g3_2+f6g2+f7g1_2+f8g0+f9g9_38;
        h[9] = f0g9+f1g8+f2g7+f3g6+f4g5+f5g4+f6g3+f7g2+f8g1+f9g0;

        // Propagation des retenues
        let mut carry = [0i64; 10];
        carry[0] = (h[0] + (1 << 25)) >> 26; h[1] += carry[0]; h[0] -= carry[0] << 26;
        carry[4] = (h[4] + (1 << 25)) >> 26; h[5] += carry[4]; h[4] -= carry[4] << 26;
        carry[1] = (h[1] + (1 << 24)) >> 25; h[2] += carry[1]; h[1] -= carry[1] << 25;
        carry[5] = (h[5] + (1 << 24)) >> 25; h[6] += carry[5]; h[5] -= carry[5] << 25;
        carry[2] = (h[2] + (1 << 25)) >> 26; h[3] += carry[2]; h[2] -= carry[2] << 26;
        carry[6] = (h[6] + (1 << 25)) >> 26; h[7] += carry[6]; h[6] -= carry[6] << 26;
        carry[3] = (h[3] + (1 << 24)) >> 25; h[4] += carry[3]; h[3] -= carry[3] << 25;
        carry[7] = (h[7] + (1 << 24)) >> 25; h[8] += carry[7]; h[7] -= carry[7] << 25;
        carry[4] = (h[4] + (1 << 25)) >> 26; h[5] += carry[4]; h[4] -= carry[4] << 26;
        carry[8] = (h[8] + (1 << 25)) >> 26; h[9] += carry[8]; h[8] -= carry[8] << 26;
        carry[9] = (h[9] + (1 << 24)) >> 25; h[0] += carry[9] * 19; h[9] -= carry[9] << 25;
        carry[0] = (h[0] + (1 << 25)) >> 26; h[1] += carry[0]; h[0] -= carry[0] << 26;
        Fe(h)
    }

    fn sq(self) -> Fe {
        self.mul(self)
    }

    /// Carré itéré n fois.
    fn sq_n(self, n: u32) -> Fe {
        let mut x = self;
        for _ in 0..n { x = x.sq(); }
        x
    }

    /// Inversion mod p via l'exposant (p-2) par la méthode d'exponentiation rapide.
    fn invert(self) -> Fe {
        // p-2 = 2^255 - 21 : addition chain précalculée
        let z1  = self;
        let z2  = z1.sq();
        let z8  = z2.sq().sq();
        let z9  = z1.mul(z8);
        let z11 = z2.mul(z9);
        let z22 = z11.sq();
        let z_5_0   = z9.mul(z22);
        let z_10_5  = z_5_0.sq_n(5);
        let z_10_0  = z_10_5.mul(z_5_0);
        let z_20_10 = z_10_0.sq_n(10);
        let z_20_0  = z_20_10.mul(z_10_0);
        let z_40_20 = z_20_0.sq_n(20);
        let z_40_0  = z_40_20.mul(z_20_0);
        let z_50_10 = z_40_0.sq_n(10);
        let z_50_0  = z_50_10.mul(z_10_0);
        let z_100_50= z_50_0.sq_n(50);
        let z_100_0 = z_100_50.mul(z_50_0);
        let z_200_100 = z_100_0.sq_n(100);
        let z_200_0 = z_200_100.mul(z_100_0);
        let z_250_50= z_200_0.sq_n(50);
        let z_250_0 = z_250_50.mul(z_50_0);
        let z_255_5 = z_250_0.sq_n(5);
        z_255_5.mul(z11)
    }

    /// Sélection conditionnelle constante en temps : if b==1 → retourne rhs, sinon self.
    fn cswap(self, other: Fe, b: u64) -> (Fe, Fe) {
        let mask = (b.wrapping_neg()) as i64;
        let mut f = self.0;
        let mut g = other.0;
        for i in 0..10 {
            let x = (f[i] ^ g[i]) & mask;
            f[i] ^= x;
            g[i] ^= x;
        }
        (Fe(f), Fe(g))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scalaire X25519 — clamping et Montgomery Ladder
// ─────────────────────────────────────────────────────────────────────────────

/// Clamp du scalaire selon RFC 7748 Section 5.
fn clamp_scalar(s: &mut [u8; 32]) {
    s[0]  &= 248;
    s[31] &= 127;
    s[31] |= 64;
}

/// Montgomery Ladder — X25519 scalaire × point u.
fn x25519_scalarmult(k_bytes: &[u8; 32], u_bytes: &[u8; 32]) -> [u8; 32] {
    let mut k = *k_bytes;
    clamp_scalar(&mut k);

    let u = Fe::from_bytes(u_bytes);

    let mut x1 = u;
    let mut x2 = Fe::ONE;
    let mut z2 = Fe::ZERO;
    let mut x3 = u;
    let mut z3 = Fe::ONE;
    let mut swap: u64 = 0;

    // Constante A24 = (486662 - 2)/4 = 121665
    let a24 = Fe([121665, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    for pos in (0..255usize).rev() {
        let b = ((k[pos >> 3] >> (pos & 7)) & 1) as u64;
        swap ^= b;
        let (nx2, nx3) = x2.cswap(x3, swap);
        x2 = nx2; x3 = nx3;
        let (nz2, nz3) = z2.cswap(z3, swap);
        z2 = nz2; z3 = nz3;
        swap = b;

        // Montgomery differential addition and doubling
        let a  = x2.add(z2);
        let aa = a.sq();
        let b2 = x2.sub(z2);
        let bb = b2.sq();
        let e  = aa.sub(bb);
        let c  = x3.add(z3);
        let d  = x3.sub(z3);
        let da = d.mul(a);
        let cb = c.mul(b2);
        let x5 = da.add(cb).sq();
        let z5 = x1.mul(da.sub(cb).sq());
        let x4 = aa.mul(bb);
        let z4 = e.mul(aa.add(a24.mul(e)));
        x3 = x5;
        z3 = z5;
        x2 = x4;
        z2 = z4;
    }

    let (nx2, nx3) = x2.cswap(x3, swap);
    x2 = nx2; let _ = nx3;
    let (nz2, _) = z2.cswap(z3, swap);
    z2 = nz2;

    x2.mul(z2.invert()).to_bytes()
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique X25519
// ─────────────────────────────────────────────────────────────────────────────

/// Paire de clés X25519.
pub struct X25519KeyPair {
    pub private_key: [u8; 32],
    pub public_key:  [u8; 32],
}

/// Base point X25519 (u = 9 encodé LE sur 32 bytes).
const X25519_BASE_POINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Points de faible ordre connus — à rejeter (RFC 7748 §6).
const LOW_ORDER_POINTS: &[[u8; 32]] = &[
    // 0
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    // 1
    [1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    // 325606250916557431795983626356110631294008115727848805560023387167927233504 (order 8)
    [0xe0,0xeb,0x7a,0x72,0x8f,0xce,0x4a,0xce,0x9e,0x2d,0x34,0xd6,0x0e,0x96,0x1e,0x74,
     0x70,0x40,0xe1,0x0d,0x9e,0x08,0x35,0xd4,0x44,0x8d,0x19,0x0c,0x85,0x2e,0xdf,0x39],
];

fn is_low_order_point(point: &[u8; 32]) -> bool {
    for lop in LOW_ORDER_POINTS {
        // Comparaison en temps constant
        let mut diff = 0u8;
        for i in 0..32 { diff |= point[i] ^ lop[i]; }
        if diff == 0 { return true; }
    }
    false
}

/// Génère une paire de clés X25519 depuis une clé privée aléatoire.
pub fn x25519_keypair_from_secret(private_key: [u8; 32]) -> X25519KeyPair {
    let mut sk = private_key;
    clamp_scalar(&mut sk);
    let pk = x25519_scalarmult(&sk, &X25519_BASE_POINT);
    X25519KeyPair { private_key: sk, public_key: pk }
}

/// Réalise le Diffie-Hellman X25519 : shared_secret = private_key × peer_public_key.
///
/// Retourne Err(LowOrderPoint) si le point reçu est de faible ordre.
pub fn x25519_diffie_hellman(
    my_private:      &[u8; 32],
    peer_public_key: &[u8; 32],
) -> Result<[u8; 32], X25519Error> {
    if is_low_order_point(peer_public_key) {
        return Err(X25519Error::LowOrderPoint);
    }
    let result = x25519_scalarmult(my_private, peer_public_key);
    // Le résultat doit aussi ne pas être un point de faible ordre
    if is_low_order_point(&result) {
        return Err(X25519Error::LowOrderPoint);
    }
    // Fence pour éviter les optimisations speculative
    compiler_fence(Ordering::SeqCst);
    Ok(result)
}
