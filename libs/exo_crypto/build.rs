//! Build script for exo_crypto - compiles PQClean C sources

use std::env;
use std::path::PathBuf;

fn main() {
    let _out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    
    println!("cargo:rerun-if-changed=vendor/pqclean");
    println!("cargo:rerun-if-changed=build.rs");
    
    // Compile ML-KEM-768 (Kyber)
    let mut kyber_build = cc::Build::new();
    kyber_build
        .include("vendor/pqclean/common")
        .include("vendor/pqclean/ml-kem-768/clean")
        .file("vendor/pqclean/ml-kem-768/clean/kem.c")
        .file("vendor/pqclean/ml-kem-768/clean/indcpa.c")
        .file("vendor/pqclean/ml-kem-768/clean/poly.c")
        .file("vendor/pqclean/ml-kem-768/clean/polyvec.c")
        .file("vendor/pqclean/ml-kem-768/clean/ntt.c")
        .file("vendor/pqclean/ml-kem-768/clean/cbd.c")
        .file("vendor/pqclean/ml-kem-768/clean/reduce.c")
        .file("vendor/pqclean/ml-kem-768/clean/verify.c")
        .file("vendor/pqclean/ml-kem-768/clean/symmetric-shake.c")
        .opt_level(3);
    
    // Compile ML-DSA-65 (Dilithium)
    let mut dilithium_build = cc::Build::new();
    dilithium_build
        .include("vendor/pqclean/common")
        .include("vendor/pqclean/ml-dsa-65/clean")
        .file("vendor/pqclean/ml-dsa-65/clean/sign.c")
        .file("vendor/pqclean/ml-dsa-65/clean/poly.c")
        .file("vendor/pqclean/ml-dsa-65/clean/polyvec.c")
        .file("vendor/pqclean/ml-dsa-65/clean/ntt.c")
        .file("vendor/pqclean/ml-dsa-65/clean/packing.c")
        .file("vendor/pqclean/ml-dsa-65/clean/reduce.c")
        .file("vendor/pqclean/ml-dsa-65/clean/rounding.c")
        .file("vendor/pqclean/ml-dsa-65/clean/symmetric-shake.c")
        .opt_level(3);
    
    // Compile common utilities (FIPS202, etc)
    let mut common_build = cc::Build::new();
    common_build
        .include("vendor/pqclean/common")
        .file("vendor/pqclean/common/fips202.c")
        .file("vendor/pqclean/common/sha2.c")
        .file("vendor/pqclean/common/aes.c")
        .file("vendor/pqclean/common/sp800-185.c")
        .opt_level(3);
    
    // Only compile if not in test mode (tests use stubs)
    #[cfg(not(feature = "stub_crypto"))]
    {
        kyber_build.compile("pqclean_kyber");
        dilithium_build.compile("pqclean_dilithium");
        common_build.compile("pqclean_common");
    }
}
