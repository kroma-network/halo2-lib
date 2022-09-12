#![allow(non_snake_case)]
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use super::pairing::PairingChip;
use super::*;
use crate::ecc::EccChip;
use crate::fields::{fp::FpStrategy, PrimeFieldChip};
use crate::gates::range::{RangeChip, RangeStrategy};
use ff::PrimeField;
use halo2_proofs::arithmetic::BaseExt;
use halo2_proofs::circuit::floor_planner::V1;
use halo2_proofs::pairing::bn256::{
    multi_miller_loop, pairing, Bn256, G1Affine, G2Affine, G2Prepared, Gt, G1, G2,
};
use halo2_proofs::pairing::group::Group;
use halo2_proofs::{
    arithmetic::FieldExt,
    circuit::{Layouter, SimpleFloorPlanner},
    dev::MockProver,
    pairing::bn256::Fr,
    plonk::*,
    poly::commitment::{Params, ParamsVerifier},
    transcript::{Blake2bRead, Blake2bWrite, Challenge255},
};
use halo2curves::bn254::Fq12;
use num_bigint::BigInt;

#[derive(Serialize, Deserialize)]
struct PairingCircuitParams {
    strategy: FpStrategy,
    degree: u32,
    num_advice: usize,
    num_lookup_advice: usize,
    num_fixed: usize,
    lookup_bits: usize,
    limb_bits: usize,
    num_limbs: usize,
}

struct PairingCircuit<F: FieldExt> {
    P: Option<G1Affine>,
    Q: Option<G2Affine>,
    _marker: PhantomData<F>,
}

impl<F: FieldExt> Default for PairingCircuit<F> {
    fn default() -> Self {
        Self { P: None, Q: None, _marker: PhantomData }
    }
}

impl<F: FieldExt> Circuit<F> for PairingCircuit<F> {
    type Config = FpConfig<F>;
    type FloorPlanner = SimpleFloorPlanner; // V1;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<F>) -> Self::Config {
        let mut folder = std::path::PathBuf::new();
        folder.push("./src/bn254");
        folder.push("pairing_circuit.config");
        let params_str = std::fs::read_to_string(folder.as_path())
            .expect("src/bn254/pairing_circuit.config file should exist");
        let params: PairingCircuitParams = serde_json::from_str(params_str.as_str()).unwrap();

        PairingChip::configure(
            meta,
            params.strategy,
            params.num_advice,
            params.num_lookup_advice,
            params.num_fixed,
            params.lookup_bits,
            params.limb_bits,
            params.num_limbs,
        )
    }

    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<F>,
    ) -> Result<(), Error> {
        let mut range_chip = RangeChip::construct(config.range_config.clone(), true);
        let mut chip = PairingChip::construct(config, &mut range_chip, true);
        chip.fp_chip.load_lookup_table(&mut layouter)?;

        let P_assigned = chip.load_private_g1(&mut layouter, self.P.clone())?;
        let Q_assigned = chip.load_private_g2(&mut layouter, self.Q.clone())?;

        /*
        // test miller loop without final exp
        {
            let f = chip.miller_loop(&mut layouter, &Q_assigned, &P_assigned)?;
            for fc in &f.coeffs {
                assert_eq!(fc.value, fc.truncation.to_bigint());
            }
            if self.P != None {
                let actual_f = multi_miller_loop(&[(
                    &self.P.unwrap(),
                    &G2Prepared::from_affine(self.Q.unwrap()),
                )]);
                let f_val: Vec<String> =
                    f.coeffs.iter().map(|x| x.value.clone().unwrap().to_str_radix(16)).collect();
                println!("single miller loop:");
                println!("actual f: {:#?}", actual_f);
                println!("circuit f: {:#?}", f_val);
            }
        }
        */

        // test optimal ate pairing
        {
            let f = chip.pairing(&mut layouter, &Q_assigned, &P_assigned)?;
            for fc in &f.coeffs {
                assert_eq!(fc.value, fc.truncation.to_bigint());
            }
            if self.P != None {
                let actual_f = pairing(&self.P.unwrap(), &self.Q.unwrap());
                let f_val: Vec<String> = f
                    .coeffs
                    .iter()
                    .map(|x| x.value.clone().unwrap().to_str_radix(16))
                    //.map(|x| x.to_bigint().clone().unwrap().to_str_radix(16))
                    .collect();
                println!("optimal ate pairing:");
                println!("actual f: {:#?}", actual_f);
                println!("circuit f: {:#?}", f_val);
            }
        }

        // IMPORTANT: this assigns all constants to the fixed columns
        // This is not optional.
        let const_rows = chip.fp_chip.range.gate.assign_and_constrain_constants(&mut layouter)?;

        // IMPORTANT: this copies cells to the lookup advice column to perform range check lookups
        // This is not optional when there is more than 1 advice column.
        chip.fp_chip.range.copy_and_lookup_cells(&mut layouter)?;

        if self.P != None {
            let num_advice = chip.fp_chip.range.gate.config.num_advice;
            let num_lookup_advice = chip.fp_chip.range.config.lookup_advice.len();
            let num_fixed = chip.fp_chip.range.gate.config.constants.len();
            let lookup_bits = chip.fp_chip.range.config.lookup_bits;
            let limb_bits = chip.fp_chip.limb_bits;
            let num_limbs = chip.fp_chip.num_limbs;

            println!("Using:\nadvice columns: {}\nspecial lookup advice columns: {}\nfixed columns: {}\nlookup bits: {}\nlimb bits: {}\nnum limbs: {}", num_advice, num_lookup_advice, num_fixed, lookup_bits, limb_bits, num_limbs);
            let advice_rows = chip.fp_chip.range.gate.advice_rows.iter();
            let horizontal_advice_rows = chip.fp_chip.range.gate.horizontal_advice_rows.iter();
            println!(
                "maximum rows used by an advice column: {}",
                std::cmp::max(
                    advice_rows.clone().max().or(Some(&0u64)).unwrap(),
                    horizontal_advice_rows.clone().max().or(Some(&0u64)).unwrap()
                )
            );
            println!(
                "minimum rows used by an advice column: {}",
                std::cmp::min(
                    advice_rows.clone().min().or(Some(&u64::MAX)).unwrap(),
                    horizontal_advice_rows.clone().min().or(Some(&u64::MAX)).unwrap()
                )
            );
            let total_cells = advice_rows.sum::<u64>() + horizontal_advice_rows.sum::<u64>() * 4;
            println!("total cells used: {}", total_cells);
            println!("cells used in special lookup column: {}", range_chip.cells_to_lookup.len());
            let total_fixed = const_rows * num_fixed;
            println!("maximum rows used by a fixed column: {}", const_rows);

            println!("Suggestions:");
            let degree = lookup_bits + 1;
            println!(
                "Have you tried using {} advice columns?",
                (total_cells + (1 << degree) - 1) / (1 << degree)
            );
            println!(
                "Have you tried using {} lookup columns?",
                (range_chip.cells_to_lookup.len() + (1 << degree) - 1) / (1 << degree)
            );
            println!(
                "Have you tried using {} fixed columns?",
                (total_fixed + (1 << degree) - 1) / (1 << degree)
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn test_pairing() {
    let mut folder = std::path::PathBuf::new();
    folder.push("./src/bn254");
    folder.push("pairing_circuit.config");
    let params_str = std::fs::read_to_string(folder.as_path())
        .expect("src/bn254/pairing_circuit.config file should exist");
    let params: PairingCircuitParams = serde_json::from_str(params_str.as_str()).unwrap();
    let k = params.degree;

    let mut rng = rand::thread_rng();

    let P = Some(G1Affine::random(&mut rng));
    let Q = Some(G2Affine::random(&mut rng));

    let circuit = PairingCircuit::<Fr> { P, Q, _marker: PhantomData };

    let prover = MockProver::run(k, &circuit, vec![]).unwrap();
    //prover.assert_satisfied();
    assert_eq!(prover.verify(), Ok(()));
}

#[cfg(test)]
#[test]
fn bench_pairing() -> Result<(), Box<dyn std::error::Error>> {
    /*
    // Parameters for FpStrategy::CustomVerticalCRT
    const DEGREE: [u32; 10] = [22, 21, 20, 19, 18, 17, 16, 15, 14, 13];
    const NUM_ADVICE: [usize; 10] = [1, 2, 3, 5, 10, 19, 37, 75, 149, 307];
    const NUM_LOOKUP: [usize; 10] = [0, 1, 1, 1, 2, 4, 7, 16, 32, 74];
    const NUM_FIXED: [usize; 10] = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
    const LOOKUP_BITS: [usize; 10] = [21, 20, 19, 18, 17, 16, 15, 14, 13, 12];
    const LIMB_BITS: [usize; 10] = [88, 88, 88, 88, 88, 88, 88, 88, 88, 88];
    */

    use std::io::BufRead;
    let mut folder = std::path::PathBuf::new();
    folder.push("./src/bn254");

    folder.push("bench_pairing.config");
    let bench_params_file = std::fs::File::open(folder.as_path())?;
    folder.pop();

    folder.push("pairing_bench.csv");
    let mut fs_results = std::fs::File::create(folder.as_path()).unwrap();
    folder.pop();
    write!(fs_results, "degree,num_advice,num_lookup,num_fixed,lookup_bits,limb_bits,num_limbs,vk_size,proof_time,proof_size,verify_time\n")?;
    folder.push("data");
    if !folder.is_dir() {
        std::fs::create_dir(folder.as_path())?;
    }

    let mut params_folder = std::path::PathBuf::new();
    params_folder.push("./params");
    if !params_folder.is_dir() {
        std::fs::create_dir(params_folder.as_path())?;
    }

    let bench_params_reader = std::io::BufReader::new(bench_params_file);
    for line in bench_params_reader.lines() {
        let bench_params: PairingCircuitParams =
            serde_json::from_str(line.unwrap().as_str()).unwrap();
        println!(
            "---------------------- degree = {} ------------------------------",
            bench_params.degree
        );
        let mut rng = rand::thread_rng();
        let start = Instant::now();

        {
            folder.pop();
            folder.push("pairing_circuit.config");
            let mut f = std::fs::File::create(folder.as_path())?;
            write!(f, "{}", serde_json::to_string(&bench_params).unwrap())?;
            folder.pop();
            folder.push("data");
        }
        let params = {
            params_folder.push(format!("bn254_{}.params", bench_params.degree));
            let fd = std::fs::File::open(params_folder.as_path());
            let params = if let Ok(mut f) = fd {
                println!("Found existing params file. Reading params...");
                Params::<G1Affine>::read(&mut f).unwrap()
            } else {
                println!("Creating new params file...");
                let mut f = std::fs::File::create(params_folder.as_path())?;
                let params = Params::<G1Affine>::unsafe_setup::<Bn256>(bench_params.degree);
                params.write(&mut f).unwrap();
                params
            };
            params_folder.pop();
            params
        };

        let circuit = PairingCircuit::<Fr>::default();
        let circuit_duration = start.elapsed();
        println!("Time elapsed in circuit & params construction: {:?}", circuit_duration);

        let vk = keygen_vk(&params, &circuit)?;
        let vk_duration = start.elapsed();
        println!("Time elapsed in generating vkey: {:?}", vk_duration - circuit_duration);
        let vk_size = {
            folder.push(format!(
                "pairing_circuit_{}_{}_{}_{}_{}_{}_{}.vkey",
                bench_params.degree,
                bench_params.num_advice,
                bench_params.num_lookup_advice,
                bench_params.num_fixed,
                bench_params.lookup_bits,
                bench_params.limb_bits,
                bench_params.num_limbs
            ));
            let mut fd = std::fs::File::create(folder.as_path()).unwrap();
            folder.pop();
            vk.write(&mut fd).unwrap();
            fd.metadata().unwrap().len()
        };
        let pk = keygen_pk(&params, vk, &circuit)?;
        let pk_duration = start.elapsed();
        println!("Time elapsed in generating pkey: {:?}", pk_duration - vk_duration);
        /*{
            folder.push(format!("pairing_circuit_{}_{}_{}_{}_{}_{}_{}.pkey", DEGREE[I], NUM_ADVICE[I], NUM_LOOKUP[I], NUM_FIXED[I], LOOKUP_BITS[I], LIMB_BITS[I], 3));
            let mut fd = std::fs::File::create(folder.as_path()).unwrap();
            folder.pop();
        }*/

        let P = Some(G1Affine::random(&mut rng));
        let Q = Some(G2Affine::random(&mut rng));
        let proof_circuit = PairingCircuit::<Fr> { P, Q, _marker: PhantomData };
        let fill_duration = start.elapsed();
        println!("Time elapsed in filling circuit: {:?}", fill_duration - pk_duration);

        // create a proof
        let mut transcript = Blake2bWrite::<_, _, Challenge255<_>>::init(vec![]);
        create_proof(&params, &pk, &[proof_circuit], &[&[]], rng, &mut transcript)?;
        let proof = transcript.finalize();
        let proof_duration = start.elapsed();
        let proof_time = proof_duration - fill_duration;
        println!("Proving time: {:?}", proof_time);

        let proof_size = {
            folder.push(format!(
                "pairing_circuit_proof_{}_{}_{}_{}_{}_{}_{}.data",
                bench_params.degree,
                bench_params.num_advice,
                bench_params.num_lookup_advice,
                bench_params.num_fixed,
                bench_params.lookup_bits,
                bench_params.limb_bits,
                bench_params.num_limbs
            ));
            let mut fd = std::fs::File::create(folder.as_path()).unwrap();
            folder.pop();
            fd.write_all(&proof).unwrap();
            fd.metadata().unwrap().len()
        };

        let verify_start = start.elapsed();
        let params_verifier: ParamsVerifier<Bn256> = params.verifier(0).unwrap();
        let strategy = SingleVerifier::new(&params_verifier);
        let mut transcript = Blake2bRead::<_, _, Challenge255<_>>::init(&proof[..]);
        assert!(
            verify_proof(&params_verifier, pk.get_vk(), strategy, &[&[]], &mut transcript).is_ok()
        );
        let verify_duration = start.elapsed();
        let verify_time = verify_duration - verify_start;
        println!("Verify time: {:?}", verify_time);

        write!(
            fs_results,
            "{},{},{},{},{},{},{},{},{:?},{},{:?}\n",
            bench_params.degree,
            bench_params.num_advice,
            bench_params.num_lookup_advice,
            bench_params.num_fixed,
            bench_params.lookup_bits,
            bench_params.limb_bits,
            bench_params.num_limbs,
            vk_size,
            proof_time,
            proof_size,
            verify_time
        )?;
    }
    Ok(())
}

#[cfg(feature = "dev-graph")]
#[test]
fn plot_pairing() {
    let k = 23;
    use plotters::prelude::*;
    create_pairing_circuit!(GateStrategy::Vertical, 1, 1, 1, 22, 88, 3);

    let root = BitMapBackend::new("layout.png", (512, 40384)).into_drawing_area();
    root.fill(&WHITE).unwrap();
    let root = root.titled("Pairing Layout", ("sans-serif", 60)).unwrap();

    let circuit = PairingCircuit::<Fr>::default();

    halo2_proofs::dev::CircuitLayout::default().render(k, &circuit, &root).unwrap();
}
