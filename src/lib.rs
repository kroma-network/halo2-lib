#![allow(unused_imports, unused_variables)]

// different memory allocator options:
// empirically jemalloc still seems to give best speeds for witness generation
#[cfg(not(target_env = "msvc"))]
use jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

//#[global_allocator]
//static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

//use mimalloc::MiMalloc;
//#[global_allocator]
//static GLOBAL: MiMalloc = MiMalloc;

pub mod bigint;
pub mod ecc;
pub mod fields;
pub mod gates;
pub mod utils;

// pub mod bn254;
// pub mod secp256k1;
