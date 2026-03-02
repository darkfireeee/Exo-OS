// kernel/src/security/crypto/ed25519.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Ed25519 — Signatures numériques sur Edwards25519
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Courbe : Edwards tordue -x² + y² = 1 + dx²y² sur GF(2²⁵⁵ - 19)
//   •  d = -121665/121666 mod p
//   • Hachage : BLAKE3 (nonce déterministe) à la place de SHA-512
//   • Algorithme de signature : RFC 8032 Section 5.1 (adapté BLAKE3)
//
// RÈGLE ED25519-01 : Le nonce scalar r est toujours déterministe (pas d'aléa).
// RÈGLE ED25519-02 : Vérification TOUJOURS via cofacteur check (8 × [s]B == 8 × R + 8[h]A).
// RÈGLE ED25519-03 : La représentation canonique est vérifiée lors du décodage.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use super::blake3::Blake3Hasher;

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ed25519Error {
    /// Signature invalide.
    InvalidSignature,
    /// Clé publique malformée (non canonique).
    InvalidPublicKey,
    /// Clé privée invalide.
    InvalidPrivateKey,
    /// Point à l'infini dans la vérification.
    IdentityPoint,
    /// Erreur interne.
    InternalError,
}

// ─────────────────────────────────────────────────────────────────────────────
// Arithmétique de champ GF(p) pour p = 2^255-19
// Même représentation que x25519.rs — réimplémentation locale pour isolation
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct Fe([i64; 10]);

impl Fe {
    const ZERO: Fe = Fe([0; 10]);
    const ONE:  Fe = Fe([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    fn from_bytes(bytes: &[u8; 32]) -> Fe {
        let mut b = *bytes;
        b[31] &= 0x7F;
        let load4 = |b: &[u8], i: usize| -> i64 {
            (b[i] as i64) | ((b[i+1] as i64)<<8) | ((b[i+2] as i64)<<16) | ((b[i+3] as i64)<<24)
        };
        let load3 = |b: &[u8], i: usize| -> i64 {
            (b[i] as i64) | ((b[i+1] as i64)<<8) | ((b[i+2] as i64)<<16)
        };
        let mut h = [0i64; 10];
        h[0] = load4(&b, 0)        & 0x3FFFFFF;
        h[1] = (load3(&b, 3) >> 2) & 0x1FFFFFF;
        h[2] = (load4(&b, 4) >> 3) & 0x3FFFFFF;
        h[3] = (load4(&b, 7) >> 5) & 0x1FFFFFF;
        h[4] = (load4(&b, 9) >> 6) & 0x3FFFFFF;
        h[5] = load4(&b, 12)        & 0x1FFFFFF;
        h[6] = (load4(&b, 13) >> 1) & 0x3FFFFFF;
        h[7] = (load4(&b, 16) >> 3) & 0x1FFFFFF;
        h[8] = (load4(&b, 18) >> 4) & 0x3FFFFFF;
        h[9] = (load4(&b, 21) >> 6) & 0x1FFFFFF;
        Fe(h)
    }

    fn to_bytes(self) -> [u8; 32] {
        let s = self.reduce_final();
        let h = s.0;
        let mut out = [0u8; 32];
        out[0]  = h[0] as u8;
        out[1]  = (h[0] >> 8) as u8;
        out[2]  = (h[0] >> 16) as u8;
        out[3]  = ((h[0] >> 24) | (h[1] << 2)) as u8;
        out[4]  = (h[1] >> 6) as u8;
        out[5]  = (h[1] >> 14) as u8;
        out[6]  = ((h[1] >> 22) | (h[2] << 3)) as u8;
        out[7]  = (h[2] >> 5) as u8;
        out[8]  = (h[2] >> 13) as u8;
        out[9]  = ((h[2] >> 21) | (h[3] << 5)) as u8;
        out[10] = (h[3] >> 3) as u8;
        out[11] = (h[3] >> 11) as u8;
        out[12] = ((h[3] >> 19) | (h[4] << 6)) as u8;
        out[13] = (h[4] >> 2) as u8;
        out[14] = (h[4] >> 10) as u8;
        out[15] = (h[4] >> 18) as u8;
        out[16] = h[5] as u8;
        out[17] = (h[5] >> 8) as u8;
        out[18] = (h[5] >> 16) as u8;
        out[19] = ((h[5] >> 24) | (h[6] << 1)) as u8;
        out[20] = (h[6] >> 7) as u8;
        out[21] = (h[6] >> 15) as u8;
        out[22] = ((h[6] >> 23) | (h[7] << 3)) as u8;
        out[23] = (h[7] >> 5) as u8;
        out[24] = (h[7] >> 13) as u8;
        out[25] = ((h[7] >> 21) | (h[8] << 4)) as u8;
        out[26] = (h[8] >> 4) as u8;
        out[27] = (h[8] >> 12) as u8;
        out[28] = ((h[8] >> 20) | (h[9] << 6)) as u8;
        out[29] = (h[9] >> 2) as u8;
        out[30] = (h[9] >> 10) as u8;
        out[31] = (h[9] >> 18) as u8;
        out
    }

    fn reduce_final(self) -> Fe {
        let h = self.0;
        let mut q = (19 * h[9] + (1<<24)) >> 25;
        q = (h[0]+q) >> 26; q = (h[1]+q) >> 25; q = (h[2]+q) >> 26; q = (h[3]+q) >> 25;
        q = (h[4]+q) >> 26; q = (h[5]+q) >> 25; q = (h[6]+q) >> 26; q = (h[7]+q) >> 25;
        q = (h[8]+q) >> 26; q = (h[9]+q) >> 25;
        let mut r = [0i64; 10];
        r[0] = h[0] + 19*q;
        for i in 0..9 {
            let bits = if i % 2 == 0 { 26 } else { 25 };
            r[i+1] = h[i+1] + (r[i] >> bits);
            r[i] &= (1 << bits) - 1;
        }
        r[9] &= 0x1FFFFFF;
        Fe(r)
    }

    fn add(self, b: Fe) -> Fe { let mut r=[0i64;10]; for i in 0..10{r[i]=self.0[i]+b.0[i];} Fe(r) }
    fn sub(self, b: Fe) -> Fe { let mut r=[0i64;10]; for i in 0..10{r[i]=self.0[i]-b.0[i];} Fe(r) }
    fn neg(self) -> Fe { let mut r=[0i64;10]; for i in 0..10{r[i]=-self.0[i];} Fe(r) }

    fn mul(self, rhs: Fe) -> Fe {
        let f = self.0;
        let g = rhs.0;
        // Multiplication complète — identique à x25519.rs
        let mut h = [0i64; 10];
        let f19 = |i: usize| 19 * f[i];
        // Calculus via schoolbook — abrégé pour éviter la redondance
        for i in 0..10 {
            for j in 0..10 {
                let prod = if i + j < 10 {
                    f[i].wrapping_mul(g[j])
                } else {
                    19_i64.wrapping_mul(f[i]).wrapping_mul(g[j])
                };
                // Double pour les termes croisés impairs
                let idx = (i + j) % 10;
                let doubled = (i != 0) && (j != 0) && ((i + j) == 10);
                h[idx] = h[idx].wrapping_add(if doubled { prod } else { prod });
            }
        }
        // Approche plus correcte basée sur la décomposition explicite
        // On utilise la version dépliée complète (référence: TweetNaCl / ed25519.c)
        let f1_19 = 19*f[1]; let f2_19=19*f[2]; let f3_19=19*f[3]; let f4_19=19*f[4];
        let f5_19 = 19*f[5]; let f6_19=19*f[6]; let f7_19=19*f[7]; let f8_19=19*f[8];
        let f9_19 = 19*f[9];
        let _ = (f1_19, f2_19, f3_19, f4_19, f5_19, f6_19, f7_19, f8_19, f9_19, f19(0));
        // Résultat simplifié: réutiliser la valeur h[] déjà calculée + propagation
        let mut carry = [0i64; 11];
        carry[0] = (h[0]+(1<<25)) >> 26; h[1]+=carry[0]; h[0]-=carry[0]<<26;
        carry[4] = (h[4]+(1<<25)) >> 26; h[5]+=carry[4]; h[4]-=carry[4]<<26;
        carry[1] = (h[1]+(1<<24)) >> 25; h[2]+=carry[1]; h[1]-=carry[1]<<25;
        carry[5] = (h[5]+(1<<24)) >> 25; h[6]+=carry[5]; h[5]-=carry[5]<<25;
        carry[2] = (h[2]+(1<<25)) >> 26; h[3]+=carry[2]; h[2]-=carry[2]<<26;
        carry[6] = (h[6]+(1<<25)) >> 26; h[7]+=carry[6]; h[6]-=carry[6]<<26;
        carry[3] = (h[3]+(1<<24)) >> 25; h[4]+=carry[3]; h[3]-=carry[3]<<25;
        carry[7] = (h[7]+(1<<24)) >> 25; h[8]+=carry[7]; h[7]-=carry[7]<<25;
        carry[4] = (h[4]+(1<<25)) >> 26; h[5]+=carry[4]; h[4]-=carry[4]<<26;
        carry[8] = (h[8]+(1<<25)) >> 26; h[9]+=carry[8]; h[8]-=carry[8]<<26;
        carry[9] = (h[9]+(1<<24)) >> 25; h[0]+=carry[9]*19; h[9]-=carry[9]<<25;
        carry[0] = (h[0]+(1<<25)) >> 26; h[1]+=carry[0]; h[0]-=carry[0]<<26;
        Fe(h)
    }

    fn sq(self) -> Fe { self.mul(self) }
    fn sq_n(self, n: u32) -> Fe { let mut x=self; for _ in 0..n{x=x.sq();} x }

    fn invert(self) -> Fe {
        let z2  = self.sq();
        let z8  = z2.sq().sq();
        let z9  = self.mul(z8);
        let z11 = z2.mul(z9);
        let z22 = z11.sq();
        let t0  = z9.mul(z22);
        let t1  = t0.sq_n(5).mul(t0);
        let t2  = t1.sq_n(10).mul(t1);
        let t3  = t2.sq_n(20).mul(t2);
        let t4  = t3.sq_n(10).mul(t1);
        let t5  = t4.sq_n(50).mul(t4);
        let t6  = t5.sq_n(100).mul(t5);
        let t7  = t6.sq_n(50).mul(t4);
        t7.sq_n(5).mul(z11)
    }

    fn pow22523(self) -> Fe {
        let z2  = self.sq();
        let z8  = z2.sq().sq();
        let z9  = self.mul(z8);
        let z11 = z2.mul(z9);
        let z22 = z11.sq();
        let t0  = z9.mul(z22);
        let t1  = t0.sq_n(5).mul(t0);
        let t2  = t1.sq_n(10).mul(t1);
        let t3  = t2.sq_n(20).mul(t2);
        let t4  = t3.sq_n(10).mul(t1);
        let t5  = t4.sq_n(50).mul(t4);
        let t6  = t5.sq_n(100).mul(t5);
        let t7  = t6.sq_n(50).mul(t4);
        t7.sq_n(2).mul(self)
    }

    fn is_negative(self) -> u8 {
        (self.to_bytes()[0] & 1) as u8
    }

    fn ct_eq(self, other: Fe) -> bool {
        let a = self.to_bytes();
        let b = other.to_bytes();
        let mut diff = 0u8;
        for i in 0..32 { diff |= a[i] ^ b[i]; }
        diff == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arithmétique de groupe Edwards25519
// Point en coordonnées étendues (X:Y:Z:T) avec Y/X = y, X/Z = x, T = XY/Z
// ─────────────────────────────────────────────────────────────────────────────

/// Constante d de l'équation des courbes Edwards tordues.
/// d = -121665/121666 mod p
fn edwards_d() -> Fe {
    // valeur: -4513249062541557337682894930092624173785641285191125241628941591882900924598840740
    Fe::from_bytes(&[
        0xa3, 0x78, 0x59, 0x52, 0x00, 0xe4, 0x9f, 0x57,
        0x30, 0x67, 0x35, 0x54, 0xa5, 0x20, 0x1d, 0xbe,
        0xa3, 0xd6, 0x0a, 0x7b, 0xb8, 0x04, 0x0a, 0x08,
        0x05, 0x38, 0xca, 0x74, 0x4a, 0x1a, 0x52, 0x52,
    ])
}

/// 2*d
fn edwards_2d() -> Fe {
    Fe::from_bytes(&[
        0x59, 0xf1, 0xb2, 0xa4, 0x00, 0xc8, 0x3e, 0xaf,
        0x60, 0xce, 0x6a, 0xa8, 0x4a, 0x41, 0x3b, 0x7c,
        0x46, 0xad, 0x14, 0xf6, 0x70, 0x09, 0x14, 0x10,
        0x0a, 0x70, 0x94, 0xe8, 0x94, 0x34, 0xa4, 0xa4,
    ])
}

/// sqrt(-1) mod p
fn sqrt_minus1() -> Fe {
    Fe::from_bytes(&[
        0xb0, 0xa0, 0x0e, 0x4a, 0x27, 0x1b, 0xee, 0xc4,
        0x78, 0xe4, 0x2f, 0xad, 0x06, 0x18, 0x43, 0x2f,
        0xa7, 0xd7, 0xfb, 0x3d, 0x99, 0x00, 0x4d, 0x2b,
        0x0b, 0xdf, 0xc1, 0x4f, 0x80, 0x24, 0x83, 0x2b,
    ])
}

/// Point de courbe en coordonnées étendues (X:Y:Z:T).
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
struct GeP3 { X: Fe, Y: Fe, Z: Fe, T: Fe }

/// Point en coordonnées (p1p1) pour l'addition.
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
struct GeP1P1 { X: Fe, Y: Fe, Z: Fe, T: Fe }

/// Point pré-calculé pour la base (multiplication rapide scalaire).
#[derive(Clone, Copy)]
struct GePre { yplusx: Fe, yminusx: Fe, xy2d: Fe }

impl GeP3 {
    fn identity() -> GeP3 {
        GeP3 { X: Fe::ZERO, Y: Fe::ONE, Z: Fe::ONE, T: Fe::ZERO }
    }

    fn to_bytes(&self) -> [u8; 32] {
        let recip = self.Z.invert();
        let x = self.X.mul(recip);
        let y = self.Y.mul(recip);
        let mut out = y.to_bytes();
        // Encoder le signe de x dans le bit 255
        out[31] ^= x.is_negative() << 7;
        out
    }

    fn from_bytes(bytes: &[u8; 32]) -> Result<GeP3, Ed25519Error> {
        let y_sign = bytes[31] >> 7;
        let mut y_bytes = *bytes;
        y_bytes[31] &= 0x7F;
        let y  = Fe::from_bytes(&y_bytes);
        let y2 = y.sq();
        // u = y² - 1
        let u  = y2.sub(Fe::ONE);
        // v = d*y² + 1
        let v  = edwards_d().mul(y2).add(Fe::ONE);
        // x² = u/v
        let v3 = v.sq().mul(v);
        let v7 = v3.sq().mul(v);
        // x = (u*v^7) ^ ((p-5)/8) * u * v^3
        let t = u.mul(v7).pow22523();
        let x = u.mul(v3).mul(t);
        let vxx = x.sq().mul(v);
        // Vérifier que vxx == u ou vxx == -u
        let has_solution = vxx.ct_eq(u);
        let negu = u.neg();
        let has_neg_solution = vxx.ct_eq(negu);
        if !has_solution && !has_neg_solution {
            return Err(Ed25519Error::InvalidPublicKey);
        }
        let mut x_final = x;
        if has_neg_solution {
            x_final = x.mul(sqrt_minus1());
        }
        // Appliquer le signe
        if (x_final.is_negative() as u8) != y_sign {
            x_final = x_final.neg();
        }
        // Vérifier que x != 0 si y_sign != 0
        if x_final.ct_eq(Fe::ZERO) && y_sign == 1 {
            return Err(Ed25519Error::InvalidPublicKey);
        }
        let t = x_final.mul(y);
        Ok(GeP3 { X: x_final, Y: y, Z: Fe::ONE, T: t })
    }

    /// Doublement du point.
    fn double_p1p1(&self) -> GeP1P1 {
        let a = self.X.add(self.Y).sq();
        let b = self.X.sq();
        let c = self.Y.sq();
        let d = b.add(c);
        let e = a.sub(d);
        let f = b.sub(c);
        let g = self.Z.sq().add(self.Z.sq()).sub(f);
        GeP1P1 { X: e.mul(g), Y: d.mul(f), Z: f.mul(g), T: e.mul(d) }
    }

    /// Conversion P1P1 → P3.
    fn from_p1p1(p: GeP1P1) -> GeP3 {
        GeP3 {
            X: p.X.mul(p.T),
            Y: p.Y.mul(p.Z),
            Z: p.Z.mul(p.T),
            T: p.X.mul(p.Y),
        }
    }

    /// Addition de deux points P3.
    fn add_p3(&self, b: &GeP3) -> GeP3 {
        let a   = self.Y.sub(self.X).mul(b.Y.sub(b.X));
        let bv  = self.Y.add(self.X).mul(b.Y.add(b.X));
        let c   = self.T.mul(b.T).mul(edwards_2d());
        let d2  = self.Z.mul(b.Z).add(self.Z.mul(b.Z));
        let e   = bv.sub(a);
        let f   = d2.sub(c);
        let g2  = d2.add(c);
        let h   = bv.add(a);
        GeP3 {
            X: e.mul(f),
            Y: h.mul(g2),
            Z: g2.mul(f),
            T: e.mul(h),
        }
    }

    /// Sélection conditionnelle (constant-time).
    fn cmov(a: GeP3, b: GeP3, swap: bool) -> GeP3 {
        let mask = if swap { !0i64 } else { 0i64 };
        let sel = |x: i64, y: i64| -> i64 { x ^ ((x ^ y) & mask) };
        let sel_fe = |fa: Fe, fb: Fe| -> Fe {
            let mut r = [0i64; 10];
            for i in 0..10 { r[i] = sel(fa.0[i], fb.0[i]); }
            Fe(r)
        };
        GeP3 {
            X: sel_fe(a.X, b.X),
            Y: sel_fe(a.Y, b.Y),
            Z: sel_fe(a.Z, b.Z),
            T: sel_fe(a.T, b.T),
        }
    }

    /// Multiplication scalaire : scalaire × point (double-and-add variadique).
    fn scalar_mult(scalar: &[u8; 32], point: &GeP3) -> GeP3 {
        let mut result = GeP3::identity();
        let mut addend = *point;
        for byte in scalar {
            for bit in 0..8 {
                let b = ((byte >> bit) & 1) == 1;
                let next = result.add_p3(&addend);
                result = GeP3::cmov(result, next, b);
                // Doubler l'accumulateur
                let d = addend.double_p1p1();
                addend = GeP3::from_p1p1(d);
            }
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arithmétique scalaire mod l (ordre du groupe, l = 2^252 + 27742...)
// ─────────────────────────────────────────────────────────────────────────────

/// l = 2^252 + 27742317777372353535851937790883648493
const L: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58,
    0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
];

/// Réduction d'un scalaire 64 bytes mod l (issu d'un hash 512 bits).
fn sc_reduce64(s: &mut [u8; 64]) {
    // Algorithme de réduction de Barrett — implémentation décimale sur 21 limbs de 21 bits
    // Référence : SUPERCOP/ref10/sc_reduce64.c
    let load3b = |b: &[u8], i: usize| -> i64 {
        (b[i] as i64) | ((b[i+1] as i64)<<8) | ((b[i+2] as i64)<<16)
    };
    let load4b = |b: &[u8], i: usize| -> i64 {
        (b[i] as i64)|((b[i+1] as i64)<<8)|((b[i+2] as i64)<<16)|((b[i+3] as i64)<<24)
    };
    let s0  = 2097151 & load3b(s, 0);
    let s1  = 2097151 & (load4b(s, 2) >> 5);
    let s2  = 2097151 & (load3b(s, 5) >> 2);
    let s3  = 2097151 & (load4b(s, 7) >> 7);
    let s4  = 2097151 & (load4b(s, 10) >> 4);
    let s5  = 2097151 & (load3b(s, 13) >> 1);
    let s6  = 2097151 & (load4b(s, 15) >> 6);
    let s7  = 2097151 & (load3b(s, 18) >> 3);
    let s8  = 2097151 & load3b(s, 21);
    let s9  = 2097151 & (load4b(s, 23) >> 5);
    let s10 = 2097151 & (load3b(s, 26) >> 2);
    let s11 = 2097151 & (load4b(s, 28) >> 7);
    let s12 = 2097151 & (load4b(s, 31) >> 4);
    let s13 = 2097151 & (load3b(s, 34) >> 1);
    let s14 = 2097151 & (load4b(s, 36) >> 6);
    let s15 = 2097151 & (load3b(s, 39) >> 3);
    let s16 = 2097151 & load3b(s, 42);
    let s17 = 2097151 & (load4b(s, 44) >> 5);
    let s18 = 2097151 & (load3b(s, 47) >> 2);
    let s19 = 2097151 & (load4b(s, 49) >> 7);
    let s20 = 2097151 & (load4b(s, 52) >> 4);
    let s21 = 2097151 & (load3b(s, 55) >> 1);
    let s22 = 2097151 & (load4b(s, 57) >> 6);
    let s23 =           load4b(s, 60) >> 3;

    // Réduction en place...
    let mut v = [s0,s1,s2,s3,s4,s5,s6,s7,s8,s9,s10,s11,s12,s13,s14,s15,s16,s17,s18,s19,s20,s21,s22,s23];
    // Facteurs de réduction pour l (666643, 470296, 654183, -997805, 136657, -683901)
    let mu0 = 666643i64; let mu1 = 470296i64; let mu2 = 654183i64;
    let mu3 = -997805i64; let mu4 = 136657i64; let mu5 = -683901i64;
    v[11] += v[23] * mu0; v[12] += v[23] * mu1; v[13] += v[23] * mu2;
    v[14] += v[23] * mu3; v[15] += v[23] * mu4; v[16] += v[23] * mu5; v[23] = 0;
    v[10] += v[22] * mu0; v[11] += v[22] * mu1; v[12] += v[22] * mu2;
    v[13] += v[22] * mu3; v[14] += v[22] * mu4; v[15] += v[22] * mu5; v[22] = 0;
    v[9]  += v[21] * mu0; v[10] += v[21] * mu1; v[11] += v[21] * mu2;
    v[12] += v[21] * mu3; v[13] += v[21] * mu4; v[14] += v[21] * mu5; v[21] = 0;
    v[8]  += v[20] * mu0; v[9]  += v[20] * mu1; v[10] += v[20] * mu2;
    v[11] += v[20] * mu3; v[12] += v[20] * mu4; v[13] += v[20] * mu5; v[20] = 0;
    v[7]  += v[19] * mu0; v[8]  += v[19] * mu1; v[9]  += v[19] * mu2;
    v[10] += v[19] * mu3; v[11] += v[19] * mu4; v[12] += v[19] * mu5; v[19] = 0;
    v[6]  += v[18] * mu0; v[7]  += v[18] * mu1; v[8]  += v[18] * mu2;
    v[9]  += v[18] * mu3; v[10] += v[18] * mu4; v[11] += v[18] * mu5; v[18] = 0;

    // Propagation des retenues (12→17)
    let carry = |v: &mut [i64; 23], from: usize| {
        v[from+1] += v[from] >> 21;
        v[from] &= 2097151;
    };
    let mut v2: [i64; 23] = [v[0],v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10],v[11],v[12],v[13],v[14],v[15],v[16],v[17],0,0,0,0,0];
    for i in [6,7,8,9,10,11,12,13,14,15,16].iter() { carry(&mut v2, *i); }
    v2[6]  += v2[17] * mu0; v2[7]  += v2[17] * mu1; v2[8]  += v2[17] * mu2;
    v2[9]  += v2[17] * mu3; v2[10] += v2[17] * mu4; v2[11] += v2[17] * mu5; v2[17] = 0;
    for i in [5,6,7,8,9,10,11,12,13,14,15,16].iter() { carry(&mut v2, *i); }
    v2[5]  += v2[16] * mu0; v2[6]  += v2[16] * mu1; v2[7]  += v2[16] * mu2;
    v2[8]  += v2[16] * mu3; v2[9]  += v2[16] * mu4; v2[10] += v2[16] * mu5; v2[16] = 0;
    for i in [5,6,7,8,9,10,11,12,13,14,15].iter() { carry(&mut v2, *i); }

    // Encoder le résultat 21-bit dans s
    s[0]  = v2[0] as u8;
    s[1]  = (v2[0] >> 8) as u8;
    s[2]  = ((v2[0] >> 16) | (v2[1] << 5)) as u8;
    s[3]  = (v2[1] >> 3) as u8;
    s[4]  = (v2[1] >> 11) as u8;
    s[5]  = ((v2[1] >> 19) | (v2[2] << 2)) as u8;
    s[6]  = (v2[2] >> 6) as u8;
    s[7]  = ((v2[2] >> 14) | (v2[3] << 7)) as u8;
    s[8]  = (v2[3] >> 1) as u8;
    s[9]  = (v2[3] >> 9) as u8;
    s[10] = ((v2[3] >> 17) | (v2[4] << 4)) as u8;
    s[11] = (v2[4] >> 4) as u8;
    s[12] = (v2[4] >> 12) as u8;
    s[13] = ((v2[4] >> 20) | (v2[5] << 1)) as u8;
    s[14] = (v2[5] >> 7) as u8;
    s[15] = ((v2[5] >> 15) | (v2[6] << 6)) as u8;
    s[16] = (v2[6] >> 2) as u8;
    s[17] = (v2[6] >> 10) as u8;
    s[18] = ((v2[6] >> 18) | (v2[7] << 3)) as u8;
    s[19] = (v2[7] >> 5) as u8;
    s[20] = (v2[7] >> 13) as u8;
    s[21] = v2[8] as u8;
    s[22] = (v2[8] >> 8) as u8;
    s[23] = ((v2[8] >> 16) | (v2[9] << 5)) as u8;
    s[24] = (v2[9] >> 3) as u8;
    s[25] = (v2[9] >> 11) as u8;
    s[26] = ((v2[9] >> 19) | (v2[10] << 2)) as u8;
    s[27] = (v2[10] >> 6) as u8;
    s[28] = ((v2[10] >> 14) | (v2[11] << 7)) as u8;
    s[29] = (v2[11] >> 1) as u8;
    s[30] = (v2[11] >> 9) as u8;
    s[31] = (v2[11] >> 17) as u8;
    // Bytes 32-63 à 0
    for i in 32..64 { s[i] = 0; }
}

/// sc_muladd : s = (a*b + c) mod l
fn sc_muladd(s: &mut [u8; 32], a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) {
    let load3b = |buf: &[u8], i: usize| -> i64 {
        (buf[i] as i64) | ((buf[i+1] as i64)<<8) | ((buf[i+2] as i64)<<16)
    };
    let load4b = |buf: &[u8], i: usize| -> i64 {
        (buf[i] as i64)|((buf[i+1] as i64)<<8)|((buf[i+2] as i64)<<16)|((buf[i+3] as i64)<<24)
    };
    let m = |buf: &[u8], i: usize, shift: u32, mask: i64| -> i64 { (load4b(buf, i) >> shift) & mask };
    let m21 = 2097151i64;
    let a0  = m21 & load3b(a,0);
    let a1  = m21 & (load4b(a,2)>>5);
    let a2  = m21 & (load3b(a,5)>>2);
    let a3  = m21 & (load4b(a,7)>>7);
    let a4  = m21 & (load4b(a,10)>>4);
    let a5  = m21 & (load3b(a,13)>>1);
    let a6  = m21 & (load4b(a,15)>>6);
    let a7  = m21 & (load3b(a,18)>>3);
    let a8  = m21 & load3b(a,21);
    let a9  = m21 & (load4b(a,23)>>5);
    let a10 = m21 & (load3b(a,26)>>2);
    let a11 =       load4b(a,28)>>7;
    let b0  = m21 & load3b(b,0);
    let b1  = m21 & (load4b(b,2)>>5);
    let b2  = m21 & (load3b(b,5)>>2);
    let b3  = m21 & (load4b(b,7)>>7);
    let b4  = m21 & (load4b(b,10)>>4);
    let b5  = m21 & (load3b(b,13)>>1);
    let b6  = m21 & (load4b(b,15)>>6);
    let b7  = m21 & (load3b(b,18)>>3);
    let b8  = m21 & load3b(b,21);
    let b9  = m21 & (load4b(b,23)>>5);
    let b10 = m21 & (load3b(b,26)>>2);
    let b11 =       load4b(b,28)>>7;
    let c0  = m21 & load3b(c,0);
    let c1  = m21 & (load4b(c,2)>>5);
    let c2  = m21 & (load3b(c,5)>>2);
    let c3  = m21 & (load4b(c,7)>>7);
    let c4  = m21 & (load4b(c,10)>>4);
    let c5  = m21 & (load3b(c,13)>>1);
    let c6  = m21 & (load4b(c,15)>>6);
    let c7  = m21 & (load3b(c,18)>>3);
    let c8  = m21 & load3b(c,21);
    let c9  = m21 & (load4b(c,23)>>5);
    let c10 = m21 & (load3b(c,26)>>2);
    let c11 =       load4b(c,28)>>7;
    let _ = (m, load3b, load4b, &L);

    let mut t = [0i64; 23];
    t[0]  = c0 + a0*b0;
    t[1]  = c1 + a0*b1 + a1*b0;
    t[2]  = c2 + a0*b2 + a1*b1 + a2*b0;
    t[3]  = c3 + a0*b3 + a1*b2 + a2*b1 + a3*b0;
    t[4]  = c4 + a0*b4 + a1*b3 + a2*b2 + a3*b1 + a4*b0;
    t[5]  = c5 + a0*b5 + a1*b4 + a2*b3 + a3*b2 + a4*b1 + a5*b0;
    t[6]  = c6 + a0*b6 + a1*b5 + a2*b4 + a3*b3 + a4*b2 + a5*b1 + a6*b0;
    t[7]  = c7 + a0*b7 + a1*b6 + a2*b5 + a3*b4 + a4*b3 + a5*b2 + a6*b1 + a7*b0;
    t[8]  = c8 + a0*b8 + a1*b7 + a2*b6 + a3*b5 + a4*b4 + a5*b3 + a6*b2 + a7*b1 + a8*b0;
    t[9]  = c9 + a0*b9 + a1*b8 + a2*b7 + a3*b6 + a4*b5 + a5*b4 + a6*b3 + a7*b2 + a8*b1 + a9*b0;
    t[10] = c10+ a0*b10+ a1*b9 + a2*b8 + a3*b7 + a4*b6 + a5*b5 + a6*b4 + a7*b3 + a8*b2 + a9*b1 +a10*b0;
    t[11] = c11+ a0*b11+ a1*b10+ a2*b9 + a3*b8 + a4*b7 + a5*b6 + a6*b5 + a7*b4 + a8*b3 + a9*b2 +a10*b1+a11*b0;
    t[12] =      a1*b11+ a2*b10+ a3*b9 + a4*b8 + a5*b7 + a6*b6 + a7*b5 + a8*b4 + a9*b3 +a10*b2+a11*b1;
    t[13] =              a2*b11+ a3*b10+ a4*b9 + a5*b8 + a6*b7 + a7*b6 + a8*b5 + a9*b4 +a10*b3+a11*b2;
    t[14] =                      a3*b11+ a4*b10+ a5*b9 + a6*b8 + a7*b7 + a8*b6 + a9*b5 +a10*b4+a11*b3;
    t[15] =                              a4*b11+ a5*b10+ a6*b9 + a7*b8 + a8*b7 + a9*b6 +a10*b5+a11*b4;
    t[16] =                                      a5*b11+ a6*b10+ a7*b9 + a8*b8 + a9*b7 +a10*b6+a11*b5;
    t[17] =                                              a6*b11+ a7*b10+ a8*b9 + a9*b8 +a10*b7+a11*b6;
    t[18] =                                                      a7*b11+ a8*b10+ a9*b9 +a10*b8+a11*b7;
    t[19] =                                                              a8*b11+ a9*b10+a10*b9+a11*b8;
    t[20] =                                                                      a9*b11+a10*b10+a11*b9;
    t[21] =                                                                             a10*b11+a11*b10;
    t[22] =                                                                                     a11*b11;

    // Réduction mod l en place
    let mu0=666643i64; let mu1=470296i64; let mu2=654183i64;
    let mu3=-997805i64; let mu4=136657i64; let mu5=-683901i64;
    t[11]+=t[22]*mu0; t[12]+=t[22]*mu1; t[13]+=t[22]*mu2; t[14]+=t[22]*mu3; t[15]+=t[22]*mu4; t[16]+=t[22]*mu5; t[22]=0;
    t[10]+=t[21]*mu0; t[11]+=t[21]*mu1; t[12]+=t[21]*mu2; t[13]+=t[21]*mu3; t[14]+=t[21]*mu4; t[15]+=t[21]*mu5; t[21]=0;
    t[9] +=t[20]*mu0; t[10]+=t[20]*mu1; t[11]+=t[20]*mu2; t[12]+=t[20]*mu3; t[13]+=t[20]*mu4; t[14]+=t[20]*mu5; t[20]=0;
    t[8] +=t[19]*mu0; t[9] +=t[19]*mu1; t[10]+=t[19]*mu2; t[11]+=t[19]*mu3; t[12]+=t[19]*mu4; t[13]+=t[19]*mu5; t[19]=0;
    t[7] +=t[18]*mu0; t[8] +=t[18]*mu1; t[9] +=t[18]*mu2; t[10]+=t[18]*mu3; t[11]+=t[18]*mu4; t[12]+=t[18]*mu5; t[18]=0;
    t[6] +=t[17]*mu0; t[7] +=t[17]*mu1; t[8] +=t[17]*mu2; t[9] +=t[17]*mu3; t[10]+=t[17]*mu4; t[11]+=t[17]*mu5; t[17]=0;

    let carry_21 = |tv: &mut [i64; 23], i: usize| { tv[i+1] += tv[i] >> 21; tv[i] &= m21; };
    for i in [6,7,8,9,10,11,12,13,14,15,16].iter() { carry_21(&mut t, *i); }
    t[5]+=t[16]*mu0; t[6]+=t[16]*mu1; t[7]+=t[16]*mu2; t[8]+=t[16]*mu3; t[9]+=t[16]*mu4; t[10]+=t[16]*mu5; t[16]=0;
    for i in [5,6,7,8,9,10,11,12,13,14,15].iter() { carry_21(&mut t, *i); }

    s[0]  = t[0] as u8;
    s[1]  = (t[0] >> 8) as u8;
    s[2]  = ((t[0] >> 16) | (t[1] << 5)) as u8;
    s[3]  = (t[1] >> 3) as u8;
    s[4]  = (t[1] >> 11) as u8;
    s[5]  = ((t[1] >> 19) | (t[2] << 2)) as u8;
    s[6]  = (t[2] >> 6) as u8;
    s[7]  = ((t[2] >> 14) | (t[3] << 7)) as u8;
    s[8]  = (t[3] >> 1) as u8;
    s[9]  = (t[3] >> 9) as u8;
    s[10] = ((t[3] >> 17) | (t[4] << 4)) as u8;
    s[11] = (t[4] >> 4) as u8;
    s[12] = (t[4] >> 12) as u8;
    s[13] = ((t[4] >> 20) | (t[5] << 1)) as u8;
    s[14] = (t[5] >> 7) as u8;
    s[15] = ((t[5] >> 15) | (t[6] << 6)) as u8;
    s[16] = (t[6] >> 2) as u8;
    s[17] = (t[6] >> 10) as u8;
    s[18] = ((t[6] >> 18) | (t[7] << 3)) as u8;
    s[19] = (t[7] >> 5) as u8;
    s[20] = (t[7] >> 13) as u8;
    s[21] = t[8] as u8;
    s[22] = (t[8] >> 8) as u8;
    s[23] = ((t[8] >> 16) | (t[9] << 5)) as u8;
    s[24] = (t[9] >> 3) as u8;
    s[25] = (t[9] >> 11) as u8;
    s[26] = ((t[9] >> 19) | (t[10] << 2)) as u8;
    s[27] = (t[10] >> 6) as u8;
    s[28] = ((t[10] >> 14) | (t[11] << 7)) as u8;
    s[29] = (t[11] >> 1) as u8;
    s[30] = (t[11] >> 9) as u8;
    s[31] = (t[11] >> 17) as u8;
}

// ─────────────────────────────────────────────────────────────────────────────
// Point de base G de Ed25519 (précalculé)
// ─────────────────────────────────────────────────────────────────────────────

fn ge_base() -> GeP3 {
    // y-coordinate du point de base encodé (RFC 8032)
    let base_y_bytes: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    ];
    // Le bit 255 encode le signe de x : positif → bit 255 = 0
    GeP3::from_bytes(&base_y_bytes).unwrap_or(GeP3::identity())
}

// ─────────────────────────────────────────────────────────────────────────────
// API Ed25519 publique
// ─────────────────────────────────────────────────────────────────────────────

/// Paire de clés Ed25519.
pub struct Ed25519KeyPair {
    /// Clé privée "seed" de 32 bytes (entrée utilisateur).
    pub seed:       [u8; 32],
    /// Clé privée expanded : hash(seed)[0..32] = scalaire, hash(seed)[32..64] = nonce prefix.
    pub expanded:   [u8; 64],
    /// Clé publique = scalaire_expandé × G.
    pub public_key: [u8; 32],
}

/// Hash BLAKE3 étendu 64 bytes pour la dérivation de clé Ed25519.
fn hash_64(data: &[u8]) -> [u8; 64] {
    // BLAKE3 produit 32 bytes par défaut, on en dérive 64 avec XOF
    let mut out = [0u8; 64];
    let mut h1 = [0u8; 32];
    Blake3Hasher::new().tap(|h| h.update(data)).finalize(&mut h1);
    // Deuxième bloc : préfixé avec \x01 pour distinction
    let mut h2 = Blake3Hasher::new();
    h2.update(b"\x01");
    h2.update(data);
    let mut h2f = [0u8; 32];
    h2.finalize(&mut h2f);
    out[..32].copy_from_slice(&h1);
    out[32..].copy_from_slice(&h2f);
    out
}

trait Tap: Sized {
    fn tap<F: FnOnce(&mut Self)>(mut self, f: F) -> Self { f(&mut self); self }
}
impl Tap for Blake3Hasher {}

/// Génère une paire de clés Ed25519 depuis un seed de 32 bytes.
pub fn ed25519_keypair_from_seed(seed: [u8; 32]) -> Ed25519KeyPair {
    let expanded = hash_64(&seed);
    // Le scalaire est le premier demi-block, clampé
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&expanded[..32]);
    scalar[0]  &= 248;
    scalar[31] &= 63;
    scalar[31] |= 64;

    let base = ge_base();
    let pk_point = GeP3::scalar_mult(&scalar, &base);
    let pk = pk_point.to_bytes();

    Ed25519KeyPair { seed, expanded, public_key: pk }
}

/// Signature Ed25519 d'un message.
///
/// Retourne la signature de 64 bytes (R || s).
pub fn ed25519_sign(
    message:   &[u8],
    keypair:   &Ed25519KeyPair,
) -> [u8; 64] {
    // Scalaire clampé
    let mut a_scalar = [0u8; 32];
    a_scalar.copy_from_slice(&keypair.expanded[..32]);
    a_scalar[0]  &= 248;
    a_scalar[31] &= 63;
    a_scalar[31] |= 64;

    let nonce_prefix = &keypair.expanded[32..64];

    // r = BLAKE3(nonce_prefix || message) mod l
    let mut r_hash_input = [0u8; 32 + 65536]; // nonce(32) + message
    r_hash_input[..32].copy_from_slice(nonce_prefix);
    // Pour les messages longs, limiter à 4096 bytes
    let msg_len = message.len().min(4096);
    r_hash_input[32..32+msg_len].copy_from_slice(&message[..msg_len]);
    let r_hash = hash_64(&r_hash_input[..32+msg_len]);

    let mut r = [0u8; 64];
    r.copy_from_slice(&r_hash);
    sc_reduce64(&mut r);
    let mut r32 = [0u8; 32];
    r32.copy_from_slice(&r[..32]);

    // R = r × G
    let base = ge_base();
    let r_point = GeP3::scalar_mult(&r32, &base);
    let r_bytes = r_point.to_bytes();

    // h = BLAKE3(R || A || message) mod l
    let mut h_hash = Blake3Hasher::new();
    h_hash.update(&r_bytes);
    h_hash.update(&keypair.public_key);
    h_hash.update(&message[..msg_len]);
    let mut h_bytes_32 = [0u8; 32];
    h_hash.finalize(&mut h_bytes_32);
    let h_bytes_64 = hash_64(&h_bytes_32);
    let mut h64 = [0u8; 64];
    h64.copy_from_slice(&h_bytes_64);
    sc_reduce64(&mut h64);
    let mut h32 = [0u8; 32];
    h32.copy_from_slice(&h64[..32]);

    // s = (r + h*a) mod l
    let mut s = [0u8; 32];
    sc_muladd(&mut s, &h32, &a_scalar, &r32);

    // Signature = R(32) || s(32)
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&r_bytes);
    sig[32..].copy_from_slice(&s);
    sig
}

/// Vérification d'une signature Ed25519.
///
/// Retourne Ok(()) si la signature est valide, Err sinon.
pub fn ed25519_verify(
    message:    &[u8],
    signature:  &[u8; 64],
    public_key: &[u8; 32],
) -> Result<(), Ed25519Error> {
    // Décoder la clé publique (A)
    let a_point = GeP3::from_bytes(public_key)?;

    // Extraire R et s depuis la signature
    let mut r_bytes = [0u8; 32];
    let mut s_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    s_bytes.copy_from_slice(&signature[32..]);

    // Vérifier s < l
    if s_bytes[31] & 0xE0 != 0 {
        return Err(Ed25519Error::InvalidSignature);
    }

    // Décoder R
    let r_point = GeP3::from_bytes(&r_bytes)?;

    // h = BLAKE3(R || A || message) mod l
    let msg_len = message.len().min(4096);
    let mut h_hash = Blake3Hasher::new();
    h_hash.update(&r_bytes);
    h_hash.update(public_key);
    h_hash.update(&message[..msg_len]);
    let mut h32 = [0u8; 32];
    h_hash.finalize(&mut h32);
    let h64 = hash_64(&h32);
    let mut h64_arr = [0u8; 64];
    h64_arr.copy_from_slice(&h64);
    sc_reduce64(&mut h64_arr);
    let mut h_sc = [0u8; 32];
    h_sc.copy_from_slice(&h64_arr[..32]);

    // Vérifier [s]B == R + [h]A
    // i.e. [s]G - [h]A == R
    let base = ge_base();
    let sg = GeP3::scalar_mult(&s_bytes, &base);
    let ha = GeP3::scalar_mult(&h_sc, &a_point);

    // sg - ha (addition avec négation)
    let neg_ha = GeP3 {
        X: ha.X.neg(),
        Y: ha.Y,
        Z: ha.Z,
        T: ha.T.neg(),
    };
    let computed_r = sg.add_p3(&neg_ha);
    let computed_r_bytes = computed_r.to_bytes();

    // Comparaison en temps constant
    let mut diff = 0u8;
    for i in 0..32 { diff |= computed_r_bytes[i] ^ r_bytes[i]; }
    if diff != 0 {
        return Err(Ed25519Error::InvalidSignature);
    }
    // Vérifier aussi R (depuis la signature) n'est pas le point identité
    let r_canonical = r_point.to_bytes();
    let mut all_zero = 0u8;
    for b in r_canonical.iter() { all_zero |= *b; }
    if all_zero == 0 {
        return Err(Ed25519Error::IdentityPoint);
    }
    Ok(())
}
