use std::fmt::Debug;

use crate::{bigint::CRTInteger, gates::RangeInstructions};
use ff::PrimeField;
use halo2_proofs::{
    arithmetic::{BaseExt, Field, FieldExt},
    circuit::{AssignedCell, Layouter},
    plonk::Error,
};
use num_bigint::BigUint;

pub mod fp;
pub mod fp_overflow;
pub mod fp12;
pub mod fp2;

#[derive(Clone, Debug)]
pub struct FqPoint<F: FieldExt> {
    // `F_q` field extension of `F_p` where `q = p^degree`
    // An `F_q` point consists of `degree` number of `F_p` points
    // The `F_p` points are stored as possibly overflow integers in CRT format

    // We do not specify the irreducible `F_p` polynomial used to construct `F_q` here - that is implementation specific
    pub coeffs: Vec<CRTInteger<F>>,
    pub degree: usize,
}

impl<F: FieldExt> FqPoint<F> {
    pub fn construct(coeffs: Vec<CRTInteger<F>>, degree: usize) -> Self {
        assert_eq!(coeffs.len(), degree);
        Self { coeffs, degree }
    }
}

/// Common functionality for finite field chips
pub trait FieldChip<F: FieldExt> {
    type ConstantType: Debug;
    type WitnessType: Debug;
    type FieldPoint: Clone + Debug;
    // a type implementing `Field` trait to help with witness generation (for example with inverse)
    type FieldType: Field;
    type RangeChip: RangeInstructions<F>;

    fn range(&mut self) -> &mut Self::RangeChip;

    fn get_assigned_value(x: &Self::FieldPoint) -> Option<Self::FieldType>;

    fn fe_to_witness(x: &Option<Self::FieldType>) -> Self::WitnessType;

    fn load_private(
        &mut self,
        layouter: &mut impl Layouter<F>,
        coeffs: Self::WitnessType,
    ) -> Result<Self::FieldPoint, Error>;

    fn load_constant(
        &mut self,
        layouter: &mut impl Layouter<F>,
        coeffs: Self::ConstantType,
    ) -> Result<Self::FieldPoint, Error>;

    fn add_no_carry(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error>;

    fn sub_no_carry(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error>;

    fn negate(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error>;

    fn scalar_mul_no_carry(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: F,
    ) -> Result<Self::FieldPoint, Error>;

    fn mul_no_carry(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error>;

    fn check_carry_mod_to_zero(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
    ) -> Result<(), Error>;

    fn carry_mod(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error>;

    fn range_check(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
    ) -> Result<(), Error>;

    fn is_zero(
	&mut self,
	layouter: &mut impl Layouter<F>,
	a: &Self::FieldPoint,
    ) -> Result<AssignedCell<F, F>, Error>;

    fn is_equal(
	&mut self,
	layouter: &mut impl Layouter<F>,
	a: &Self::FieldPoint,
	b: &Self::FieldPoint,
    ) -> Result<AssignedCell<F, F>, Error>;
    
    fn mul(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error> {
        let no_carry = self.mul_no_carry(layouter, a, b)?;
        self.carry_mod(layouter, &no_carry)
    }

    fn divide(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error> {	
        let a_val = Self::get_assigned_value(a);
        let b_val = Self::get_assigned_value(b);
        let b_inv: Option<Self::FieldType> =
            if let Some(bv) = b_val { bv.invert().into() } else { None };
        let quot_val = a_val.zip(b_inv).map(|(a, bi)| a * bi);

        let quot = self.load_private(layouter, Self::fe_to_witness(&quot_val))?;
        self.range_check(layouter, &quot)?;

	println!("a_val {:?}", a_val);
	println!("b_val {:?}", b_val);
	println!("b_inv {:?}", b_inv);
	println!("quot {:?}", quot_val);
	
        // constrain quot * b - a = 0 mod p
        let quot_b = self.mul_no_carry(layouter, &quot, b)?;
        let quot_constraint = self.sub_no_carry(layouter, &quot_b, a)?;
        self.check_carry_mod_to_zero(layouter, &quot_constraint)?;

        Ok(quot)
    }

    // constrain and output -a / b
    // this is usually cheaper constraint-wise than computing -a and then (-a) / b separately
    fn neg_divide(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::FieldPoint,
        b: &Self::FieldPoint,
    ) -> Result<Self::FieldPoint, Error> {
        let a_val = Self::get_assigned_value(a);
        let b_val = Self::get_assigned_value(b);
        let b_inv: Option<Self::FieldType> =
            if let Some(bv) = b_val { bv.invert().into() } else { None };
        let quot_val = a_val.zip(b_inv).map(|(a, b)| -a * b);

        let quot = self.load_private(layouter, Self::fe_to_witness(&quot_val))?;
        self.range_check(layouter, &quot)?;

        // constrain quot * b + a = 0 mod p
        let quot_b = self.mul_no_carry(layouter, &quot, b)?;
        let quot_constraint = self.add_no_carry(layouter, &quot_b, a)?;
        self.check_carry_mod_to_zero(layouter, &quot_constraint)?;

        Ok(quot)
    }
}

pub trait Selectable<F: FieldExt> {
    type Point;

    fn select(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Self::Point,
        b: &Self::Point,
        sel: &AssignedCell<F, F>,
    ) -> Result<Self::Point, Error>;

    fn inner_product(
        &mut self,
        layouter: &mut impl Layouter<F>,
        a: &Vec<Self::Point>,
        coeffs: &Vec<AssignedCell<F, F>>,
    ) -> Result<Self::Point, Error>;
}

// Common functionality for prime field chips
pub trait PrimeFieldChip<F: FieldExt>: FieldChip<F> {
    type Config;

    fn construct(config: Self::Config, using_simple_floor_planner: bool) -> Self;
}

// helper trait so we can actually construct and read the Fp2 struct
// needs to be implemented for Fp2 struct for use cases below
pub trait FieldExtConstructor<Fp: PrimeField, const DEGREE: usize> {
    fn new(c: [Fp; DEGREE]) -> Self;

    fn coeffs(&self) -> Vec<Fp>;
}
