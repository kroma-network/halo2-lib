#![allow(non_snake_case)]
use crate::bigint::CRTInteger;
use crate::fields::{fp::FpConfig, FieldChip, PrimeFieldChip, Selectable};
use crate::halo2_proofs::{arithmetic::CurveAffine, circuit::Value};
use ff::Field;
use group::{Curve, Group};
use halo2_base::halo2_proofs::halo2curves::bn256::{Bn256, G1Affine};
use halo2_base::{
    gates::{GateInstructions, RangeInstructions},
    utils::{modulus, CurveAffineExt, PrimeField},
    AssignedValue, Context,
    QuantumCell::Existing,
};
use itertools::Itertools;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use std::marker::PhantomData;

pub mod ecdsa;
pub mod fixed_base;
// pub mod fixed_base_pippenger;
pub mod pippenger;

// EcPoint and EccChip take in a generic `FieldChip` to implement generic elliptic curve operations on arbitrary field extensions (provided chip exists) for short Weierstrass curves (currently further assuming a4 = 0 for optimization purposes)
#[derive(Debug)]
pub struct EcPoint<F: PrimeField, FieldPoint: Clone> {
    pub x: FieldPoint,
    pub y: FieldPoint,
    _marker: PhantomData<F>,
}

impl<F: PrimeField, FieldPoint: Clone> Clone for EcPoint<F, FieldPoint> {
    fn clone(&self) -> Self {
        Self { x: self.x.clone(), y: self.y.clone(), _marker: PhantomData }
    }
}

impl<F: PrimeField, FieldPoint: Clone> EcPoint<F, FieldPoint> {
    pub fn construct(x: FieldPoint, y: FieldPoint) -> Self {
        Self { x, y, _marker: PhantomData }
    }

    pub fn x(&self) -> &FieldPoint {
        &self.x
    }

    pub fn y(&self) -> &FieldPoint {
        &self.y
    }
}

fn get_value<F: Default + Clone>(a: Value<F>) -> F {
    let mut t = F::default();
    a.map(|x| t = x);
    t
}

pub fn ec_add_unequal<'v, F: PrimeField, FC: FieldChip<F>>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
    Q: &EcPoint<F, FC::FieldPoint<'v>>,
    is_strict: bool,
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC::FieldType: From<u64>,
{
    // let R2 = ec_add_unequal_old(chip, ctx, P, Q, is_strict);
    println!("using the new formula");
    // compute the result R := (rx, ry) = P + Q in the clear
    let R: EcPoint<F, FC::FieldPoint<'v>> = {
        let px = get_value(chip.get_assigned_value(P.x()));
        let py = get_value(chip.get_assigned_value(P.y()));
        let qx = get_value(chip.get_assigned_value(Q.x()));
        let qy = get_value(chip.get_assigned_value(Q.y()));

        assert_ne!(px, qx);
        assert_ne!(py, qy);

        // let rx2 = get_value(chip.get_assigned_value(R2.x()));
        // let ry2 = get_value(chip.get_assigned_value(R2.y()));

        //  lambda = (y_2-y_1)/(x_2-x_1)
        let lambda = (qy - py) * (qx - px).invert().unwrap();
        //  x_3 = lambda^2 - x_1 - x_2 (mod p)
        let rx = lambda * lambda - px - qx;
        //  y_3 = lambda (x_1 - x_3) - y_1 mod p
        let ry = lambda * (px - rx) - py;
        // assert_eq!(rx, rx2);
        // assert_eq!(ry, ry2);

        let rx_wire = chip.load_private(ctx, FC::fe_to_witness(&Value::known(rx)));
        let ry_wire = chip.load_private(ctx, FC::fe_to_witness(&Value::known(ry)));
        EcPoint::construct(rx_wire, ry_wire)
    };

    // check R is on curve
    {
        let lhs = chip.mul_no_carry(ctx, &R.y, &R.y);
        let mut rhs = chip.mul(ctx, &R.x, &R.x);
        rhs = chip.mul_no_carry(ctx, &rhs, &R.x);
        // hard code for bn curve -- fixme for other curves
        let b = FC::fe_to_constant(<FC as FieldChip<F>>::FieldType::from(3u64));
        rhs = chip.add_constant_no_carry(ctx, &rhs, b);
        let diff = chip.sub_no_carry(ctx, &lhs, &rhs);
        chip.check_carry_mod_to_zero(ctx, &diff);
    }
    // check (y3 - y1)(x2 - x1) = (y2 - y1)(x3 - x1)
    {
        // (y3 - y1)(x2 - x1)
        let mut lhs = chip.sub_no_carry(ctx, &R.y, &P.y);
        let tmp = chip.sub_no_carry(ctx, &Q.x, &P.x);
        lhs = chip.mul_no_carry(ctx, &lhs, &tmp);
        // (y2 - y1)(x3 - x1)
        let mut rhs = chip.sub_no_carry(ctx, &Q.y, &P.y);
        let tmp = chip.sub_no_carry(ctx, &R.x, &P.x);
        rhs = chip.mul_no_carry(ctx, &rhs, &tmp);

        let diff = chip.sub_no_carry(ctx, &lhs, &rhs);
        chip.check_carry_mod_to_zero(ctx, &diff);
    }

    R
}

// Implements:
//  Given P = (x_1, y_1) and Q = (x_2, y_2), ecc points over the field F_p
//      assume x_1 != x_2
//  Find ec addition P + Q = (x_3, y_3)
// By solving:
//  lambda = (y_2-y_1)/(x_2-x_1) using constraint
//  lambda * (x_2 - x_1) = y_2 - y_1
//  x_3 = lambda^2 - x_1 - x_2 (mod p)
//  y_3 = lambda (x_1 - x_3) - y_1 mod p
//
/// For optimization reasons, we assume that if you are using this with `is_strict = true`, then you have already called `chip.enforce_less_than_p` on both `P.x` and `P.y`
pub fn ec_add_unequal_old<'v, F: PrimeField, FC: FieldChip<F>>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
    Q: &EcPoint<F, FC::FieldPoint<'v>>,
    is_strict: bool,
) -> EcPoint<F, FC::FieldPoint<'v>> {
    if is_strict {
        // constrains that P.x != Q.x
        let x_is_equal = chip.is_equal_unenforced(ctx, &P.x, &Q.x);
        chip.range().gate().assert_is_const(ctx, &x_is_equal, F::zero());
    }

    let dx = chip.sub_no_carry(ctx, &Q.x, &P.x);
    let dy = chip.sub_no_carry(ctx, &Q.y, &P.y);
    let lambda = chip.divide(ctx, &dy, &dx);

    //  x_3 = lambda^2 - x_1 - x_2 (mod p)
    let lambda_sq = chip.mul_no_carry(ctx, &lambda, &lambda);
    let lambda_sq_minus_px = chip.sub_no_carry(ctx, &lambda_sq, &P.x);
    let x_3_no_carry = chip.sub_no_carry(ctx, &lambda_sq_minus_px, &Q.x);
    let x_3 = chip.carry_mod(ctx, &x_3_no_carry);

    //  y_3 = lambda (x_1 - x_3) - y_1 mod p
    let dx_13 = chip.sub_no_carry(ctx, &P.x, &x_3);
    let lambda_dx_13 = chip.mul_no_carry(ctx, &lambda, &dx_13);
    let y_3_no_carry = chip.sub_no_carry(ctx, &lambda_dx_13, &P.y);
    let y_3 = chip.carry_mod(ctx, &y_3_no_carry);

    EcPoint::construct(x_3, y_3)
}

// Implements:
//  Given P = (x_1, y_1) and Q = (x_2, y_2), ecc points over the field F_p
//  Find ecc subtraction P - Q = (x_3, y_3)
//  -Q = (x_2, -y_2)
//  lambda = -(y_2+y_1)/(x_2-x_1) using constraint
//  x_3 = lambda^2 - x_1 - x_2 (mod p)
//  y_3 = lambda (x_1 - x_3) - y_1 mod p
//  Assumes that P !=Q and Q != (P - Q)
//
/// For optimization reasons, we assume that if you are using this with `is_strict = true`, then you have already called `chip.enforce_less_than_p` on both `P.x` and `P.y`
pub fn ec_sub_unequal<'v, F: PrimeField, FC: FieldChip<F>>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
    Q: &EcPoint<F, FC::FieldPoint<'v>>,
    is_strict: bool,
) -> EcPoint<F, FC::FieldPoint<'v>> {
    if is_strict {
        // constrains that P.x != Q.x
        let x_is_equal = chip.is_equal_unenforced(ctx, &P.x, &Q.x);
        chip.range().gate().assert_is_const(ctx, &x_is_equal, F::zero());
    }

    let dx = chip.sub_no_carry(ctx, &Q.x, &P.x);
    let dy = chip.add_no_carry(ctx, &Q.y, &P.y);

    let lambda = chip.neg_divide(ctx, &dy, &dx);

    // (x_2 - x_1) * lambda + y_2 + y_1 = 0 (mod p)
    let lambda_dx = chip.mul_no_carry(ctx, &lambda, &dx);
    let lambda_dx_plus_dy = chip.add_no_carry(ctx, &lambda_dx, &dy);
    chip.check_carry_mod_to_zero(ctx, &lambda_dx_plus_dy);

    //  x_3 = lambda^2 - x_1 - x_2 (mod p)
    let lambda_sq = chip.mul_no_carry(ctx, &lambda, &lambda);
    let lambda_sq_minus_px = chip.sub_no_carry(ctx, &lambda_sq, &P.x);
    let x_3_no_carry = chip.sub_no_carry(ctx, &lambda_sq_minus_px, &Q.x);
    let x_3 = chip.carry_mod(ctx, &x_3_no_carry);

    //  y_3 = lambda (x_1 - x_3) - y_1 mod p
    let dx_13 = chip.sub_no_carry(ctx, &P.x, &x_3);
    let lambda_dx_13 = chip.mul_no_carry(ctx, &lambda, &dx_13);
    let y_3_no_carry = chip.sub_no_carry(ctx, &lambda_dx_13, &P.y);
    let y_3 = chip.carry_mod(ctx, &y_3_no_carry);

    EcPoint::construct(x_3, y_3)
}

// Implements:
// computing 2P on elliptic curve E for P = (x, y)
// formula from https://crypto.stanford.edu/pbc/notes/elliptic/explicit.html
// assume y != 0 (otherwise 2P = O)

// lamb =  3x^2 / (2 y) % p
// x_3 = out[0] = lambda^2 - 2 x % p
// y_3 = out[1] = lambda (x - x_3) - y % p

// we precompute lambda and constrain (2y) * lambda = 3 x^2 (mod p)
// then we compute x_3 = lambda^2 - 2 x (mod p)
//                 y_3 = lambda (x - x_3) - y (mod p)
pub fn ec_double<'v, F: PrimeField, FC: FieldChip<F>>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
) -> EcPoint<F, FC::FieldPoint<'v>> {
    // removed optimization that computes `2 * lambda` while assigning witness to `lambda` simultaneously, in favor of readability. The difference is just copying `lambda` once
    let two_y = chip.scalar_mul_no_carry(ctx, &P.y, 2);
    let three_x = chip.scalar_mul_no_carry(ctx, &P.x, 3);
    let three_x_sq = chip.mul_no_carry(ctx, &three_x, &P.x);
    let lambda = chip.divide(ctx, &three_x_sq, &two_y);

    // x_3 = lambda^2 - 2 x % p
    let lambda_sq = chip.mul_no_carry(ctx, &lambda, &lambda);
    let two_x = chip.scalar_mul_no_carry(ctx, &P.x, 2);
    let x_3_no_carry = chip.sub_no_carry(ctx, &lambda_sq, &two_x);
    let x_3 = chip.carry_mod(ctx, &x_3_no_carry);

    // y_3 = lambda (x - x_3) - y % p
    let dx = chip.sub_no_carry(ctx, &P.x, &x_3);
    let lambda_dx = chip.mul_no_carry(ctx, &lambda, &dx);
    let y_3_no_carry = chip.sub_no_carry(ctx, &lambda_dx, &P.y);
    let y_3 = chip.carry_mod(ctx, &y_3_no_carry);

    EcPoint::construct(x_3, y_3)
}

pub fn ec_select<'v, F: PrimeField, FC>(
    chip: &FC,
    ctx: &mut Context<'_, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
    Q: &EcPoint<F, FC::FieldPoint<'v>>,
    sel: &AssignedValue<'v, F>,
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC: FieldChip<F> + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
{
    let Rx = chip.select(ctx, &P.x, &Q.x, sel);
    let Ry = chip.select(ctx, &P.y, &Q.y, sel);
    EcPoint::construct(Rx, Ry)
}

// takes the dot product of points with sel, where each is intepreted as
// a _vector_
pub fn ec_select_by_indicator<'v, F: PrimeField, FC>(
    chip: &FC,
    ctx: &mut Context<'_, F>,
    points: &[EcPoint<F, FC::FieldPoint<'v>>],
    coeffs: &[AssignedValue<'v, F>],
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC: FieldChip<F> + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
{
    let x_coords = points.iter().map(|P| P.x.clone()).collect::<Vec<_>>();
    let y_coords = points.iter().map(|P| P.y.clone()).collect::<Vec<_>>();
    let Rx = chip.select_by_indicator(ctx, &x_coords, coeffs);
    let Ry = chip.select_by_indicator(ctx, &y_coords, coeffs);
    EcPoint::construct(Rx, Ry)
}

// `sel` is little-endian binary
pub fn ec_select_from_bits<'v, F: PrimeField, FC>(
    chip: &FC,
    ctx: &mut Context<'_, F>,
    points: &[EcPoint<F, FC::FieldPoint<'v>>],
    sel: &[AssignedValue<'v, F>],
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC: FieldChip<F> + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
{
    let w = sel.len();
    let num_points = points.len();
    assert_eq!(1 << w, num_points);
    let coeffs = chip.range().gate().bits_to_indicator(ctx, sel);
    ec_select_by_indicator(chip, ctx, points, &coeffs)
}

// computes [scalar] * P on y^2 = x^3 + b
// - `scalar` is represented as a reference array of `AssignedCell`s
// - `scalar = sum_i scalar_i * 2^{max_bits * i}`
// - an array of length > 1 is needed when `scalar` exceeds the modulus of scalar field `F`
// assumes:
// - `scalar_i < 2^{max_bits} for all i` (constrained by num_to_bits)
// - `max_bits <= modulus::<F>.bits()`
//   * P has order given by the scalar field modulus
pub fn scalar_multiply<'v, F: PrimeField, FC>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
    scalar: &Vec<AssignedValue<'v, F>>,
    max_bits: usize,
    window_bits: usize,
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC: FieldChip<F> + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
    FC::FieldType: From<u64>,
{
    assert!(!scalar.is_empty());
    assert!((max_bits as u64) <= modulus::<F>().bits());

    let total_bits = max_bits * scalar.len();
    let num_windows = (total_bits + window_bits - 1) / window_bits;
    let rounded_bitlen = num_windows * window_bits;

    let mut bits = Vec::with_capacity(rounded_bitlen);
    for x in scalar {
        let mut new_bits = chip.gate().num_to_bits(ctx, x, max_bits);
        bits.append(&mut new_bits);
    }
    let mut rounded_bits = bits;
    let zero_cell = chip.gate().load_zero(ctx);
    for _ in 0..(rounded_bitlen - total_bits) {
        rounded_bits.push(zero_cell.clone());
    }

    // is_started[idx] holds whether there is a 1 in bits with index at least (rounded_bitlen - idx)
    let mut is_started = Vec::with_capacity(rounded_bitlen);
    for _ in 0..(rounded_bitlen - total_bits) {
        is_started.push(zero_cell.clone());
    }
    is_started.push(zero_cell.clone());
    for idx in 1..total_bits {
        let or = chip.gate().or(
            ctx,
            Existing(&is_started[rounded_bitlen - total_bits + idx - 1]),
            Existing(&rounded_bits[total_bits - idx]),
        );
        is_started.push(or.clone());
    }

    // is_zero_window[idx] is 0/1 depending on whether bits [rounded_bitlen - window_bits * (idx + 1), rounded_bitlen - window_bits * idx) are all 0
    let mut is_zero_window = Vec::with_capacity(num_windows);
    for idx in 0..num_windows {
        let temp_bits = rounded_bits
            [rounded_bitlen - window_bits * (idx + 1)..rounded_bitlen - window_bits * idx]
            .iter()
            .map(|x| Existing(x));
        let bit_sum = chip.gate().sum(ctx, temp_bits);
        let is_zero = chip.gate().is_zero(ctx, &bit_sum);
        is_zero_window.push(is_zero.clone());
    }

    // cached_points[idx] stores idx * P, with cached_points[0] = P
    let cache_size = 1usize << window_bits;
    let mut cached_points = Vec::with_capacity(cache_size);
    cached_points.push(P.clone());
    cached_points.push(P.clone());
    for idx in 2..cache_size {
        if idx == 2 {
            let double = ec_double(chip, ctx, P /*, b*/);
            cached_points.push(double.clone());
        } else {
            let new_point = ec_add_unequal(chip, ctx, &cached_points[idx - 1], P, false);
            cached_points.push(new_point.clone());
        }
    }

    // if all the starting window bits are 0, get start_point = P
    let mut curr_point = ec_select_from_bits::<F, FC>(
        chip,
        ctx,
        &cached_points,
        &rounded_bits[rounded_bitlen - window_bits..rounded_bitlen],
    );

    for idx in 1..num_windows {
        let mut mult_point = curr_point.clone();
        for _ in 0..window_bits {
            mult_point = ec_double(chip, ctx, &mult_point);
        }
        let add_point = ec_select_from_bits::<F, FC>(
            chip,
            ctx,
            &cached_points,
            &rounded_bits
                [rounded_bitlen - window_bits * (idx + 1)..rounded_bitlen - window_bits * idx],
        );
        let mult_and_add = ec_add_unequal(chip, ctx, &mult_point, &add_point, false);
        let is_started_point =
            ec_select(chip, ctx, &mult_point, &mult_and_add, &is_zero_window[idx]);

        curr_point =
            ec_select(chip, ctx, &is_started_point, &add_point, &is_started[window_bits * idx]);
    }
    curr_point
}

pub fn is_on_curve<'v, F, FC, C>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &EcPoint<F, FC::FieldPoint<'v>>,
) where
    F: PrimeField,
    FC: FieldChip<F>,
    C: CurveAffine<Base = FC::FieldType>,
{
    let lhs = chip.mul_no_carry(ctx, &P.y, &P.y);
    let mut rhs = chip.mul(ctx, &P.x, &P.x);
    rhs = chip.mul_no_carry(ctx, &rhs, &P.x);

    let b = FC::fe_to_constant(C::b());
    rhs = chip.add_constant_no_carry(ctx, &rhs, b);
    let diff = chip.sub_no_carry(ctx, &lhs, &rhs);
    chip.check_carry_mod_to_zero(ctx, &diff)
}

pub fn load_random_point<'v, F, FC, C>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    F: PrimeField,
    FC: FieldChip<F>,
    C: CurveAffineExt<Base = FC::FieldType>,
{
    let base_point: C = C::CurveExt::random(ChaCha20Rng::from_entropy()).to_affine();
    let (x, y) = base_point.into_coordinates();
    let pt_x = FC::fe_to_witness(&Value::known(x));
    let pt_y = FC::fe_to_witness(&Value::known(y));
    let base = {
        let x_overflow = chip.load_private(ctx, pt_x);
        let y_overflow = chip.load_private(ctx, pt_y);
        EcPoint::construct(x_overflow, y_overflow)
    };
    // for above reason we still need to constrain that the witness is on the curve
    is_on_curve::<F, FC, C>(chip, ctx, &base);
    base
}

// need to supply an extra generic `C` implementing `CurveAffine` trait in order to generate random witness points on the curve in question
// Using Simultaneous 2^w-Ary Method, see https://www.bmoeller.de/pdf/multiexp-sac2001.pdf
// Random Accumlation point trick learned from halo2wrong: https://hackmd.io/ncuKqRXzR-Cw-Au2fGzsMg?view
// Input:
// - `scalars` is vector of same length as `P`
// - each `scalar` in `scalars` satisfies same assumptions as in `scalar_multiply` above
pub fn multi_scalar_multiply<'v, F: PrimeField, FC, C>(
    chip: &FC,
    ctx: &mut Context<'v, F>,
    P: &[EcPoint<F, FC::FieldPoint<'v>>],
    scalars: &[Vec<AssignedValue<'v, F>>],
    max_bits: usize,
    window_bits: usize,
) -> EcPoint<F, FC::FieldPoint<'v>>
where
    FC: FieldChip<F> + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
    C: CurveAffineExt<Base = FC::FieldType>,
    FC::FieldType: From<u64>,
{
    let k = P.len();
    assert_eq!(k, scalars.len());
    assert_ne!(k, 0);
    assert!(!scalars[0].is_empty());
    assert!((max_bits as u32) <= F::NUM_BITS);

    let scalar_len = scalars[0].len();
    let total_bits = max_bits * scalar_len;
    let num_windows = (total_bits + window_bits - 1) / window_bits;
    let rounded_bitlen = num_windows * window_bits;

    let zero_cell = chip.gate().load_zero(ctx);
    let rounded_bits = scalars
        .iter()
        .flat_map(|scalar| {
            assert_eq!(scalar.len(), scalar_len);
            scalar
                .iter()
                .flat_map(|scalar_chunk| chip.gate().num_to_bits(ctx, scalar_chunk, max_bits))
                .chain(
                    std::iter::repeat_with(|| zero_cell.clone()).take(rounded_bitlen - total_bits),
                )
                .collect_vec()
        })
        .collect_vec();

    // load random C point as witness
    // note that while we load a random point, an adversary would load a specifically chosen point, so we must carefully handle edge cases with constraints
    let base = load_random_point::<F, FC, C>(chip, ctx);
    // contains random base points [A, ..., 2^{w + k - 1} * A]
    let mut rand_start_vec = Vec::with_capacity(k + window_bits);
    rand_start_vec.push(base);
    for idx in 1..(k + window_bits) {
        let base_mult = ec_double(chip, ctx, &rand_start_vec[idx - 1]);
        rand_start_vec.push(base_mult);
    }
    assert!(rand_start_vec.len() >= k + window_bits);

    let cache_size = 1usize << window_bits;
    // this is really a 2d array that we store as 1d vec for memory optimization
    let mut cached_points = Vec::with_capacity(k * cache_size);
    for (idx, point) in P.iter().enumerate() {
        // add selector for whether P_i is the point at infinity (aka 0 in elliptic curve group)
        // this can be checked by P_i.y == 0 iff P_i == O
        let is_infinity = chip.is_zero(ctx, &point.y);
        // (1 - 2^w) * [A, ..., 2^(k - 1) * A]
        let neg_mult_rand_start = ec_sub_unequal(
            chip,
            ctx,
            &rand_start_vec[idx],
            &rand_start_vec[idx + window_bits],
            false,
        );
        chip.enforce_less_than(ctx, point.x());
        chip.enforce_less_than(ctx, neg_mult_rand_start.x());
        // cached_points[i][0..cache_size] stores (1 - 2^w) * 2^i * A + [0..cache_size] * P_i
        cached_points.push(neg_mult_rand_start);
        for _ in 0..(cache_size - 1) {
            let prev = cached_points.last().unwrap();
            // adversary could pick `A` so add equal case occurs, so we must use strict add_unequal
            let mut new_point = ec_add_unequal(chip, ctx, prev, point, true);
            // special case for when P[idx] = O
            new_point = ec_select(chip, ctx, prev, &new_point, &is_infinity);
            chip.enforce_less_than(ctx, new_point.x());
            cached_points.push(new_point);
        }
    }

    // initialize at (2^{k + 1} - 1) * A
    // note k can be large (e.g., 800) so 2^{k+1} may be larger than the order of A
    // random fact: 2^{k + 1} - 1 can be prime: see Mersenne primes
    // TODO: I don't see a way to rule out 2^{k+1} A = +-A case in general, so will use strict sub_unequal
    let start_point = if k < F::CAPACITY as usize {
        ec_sub_unequal(chip, ctx, &rand_start_vec[k], &rand_start_vec[0], false)
    } else {
        chip.enforce_less_than(ctx, rand_start_vec[k].x());
        chip.enforce_less_than(ctx, rand_start_vec[0].x());
        ec_sub_unequal(chip, ctx, &rand_start_vec[k], &rand_start_vec[0], true)
    };
    let mut curr_point = start_point.clone();

    // compute \sum_i x_i P_i + (2^{k + 1} - 1) * A
    for idx in 0..num_windows {
        for _ in 0..window_bits {
            curr_point = ec_double(chip, ctx, &curr_point);
        }
        for (cached_points, rounded_bits) in
            cached_points.chunks(cache_size).zip(rounded_bits.chunks(rounded_bitlen))
        {
            let add_point = ec_select_from_bits::<F, FC>(
                chip,
                ctx,
                cached_points,
                &rounded_bits
                    [rounded_bitlen - window_bits * (idx + 1)..rounded_bitlen - window_bits * idx],
            );
            chip.enforce_less_than(ctx, curr_point.x());
            // this all needs strict add_unequal since A can be non-randomly chosen by adversary
            curr_point = ec_add_unequal(chip, ctx, &curr_point, &add_point, true);
        }
    }
    chip.enforce_less_than(ctx, start_point.x());
    chip.enforce_less_than(ctx, curr_point.x());
    ec_sub_unequal(chip, ctx, &curr_point, &start_point, true)
}

pub fn get_naf(mut exp: Vec<u64>) -> Vec<i8> {
    // https://en.wikipedia.org/wiki/Non-adjacent_form
    // NAF for exp:
    let mut naf: Vec<i8> = Vec::with_capacity(64 * exp.len());
    let len = exp.len();

    // generate the NAF for exp
    for idx in 0..len {
        let mut e: u64 = exp[idx];
        for _ in 0..64 {
            if e & 1 == 1 {
                let z = 2i8 - (e % 4) as i8;
                e /= 2;
                if z == -1 {
                    e += 1;
                }
                naf.push(z);
            } else {
                naf.push(0);
                e /= 2;
            }
        }
        if e != 0 {
            assert_eq!(e, 1);
            let mut j = idx + 1;
            while j < exp.len() && exp[j] == u64::MAX {
                exp[j] = 0;
                j += 1;
            }
            if j < exp.len() {
                exp[j] += 1;
            } else {
                exp.push(1);
            }
        }
    }
    if exp.len() != len {
        assert_eq!(len, exp.len() + 1);
        assert!(exp[len] == 1);
        naf.push(1);
    }
    naf
}

pub type BaseFieldEccChip<C> = EccChip<
    <C as CurveAffine>::ScalarExt,
    FpConfig<<C as CurveAffine>::ScalarExt, <C as CurveAffine>::Base>,
>;

#[derive(Clone, Debug)]
pub struct EccChip<F: PrimeField, FC: FieldChip<F>> {
    pub field_chip: FC,
    _marker: PhantomData<F>,
}

impl<F: PrimeField, FC: FieldChip<F>> EccChip<F, FC> {
    pub fn construct(field_chip: FC) -> Self {
        Self { field_chip, _marker: PhantomData }
    }

    pub fn field_chip(&self) -> &FC {
        &self.field_chip
    }

    pub fn load_private<'v>(
        &self,
        ctx: &mut Context<'_, F>,
        point: (Value<FC::FieldType>, Value<FC::FieldType>),
    ) -> EcPoint<F, FC::FieldPoint<'v>> {
        let (x, y) = (FC::fe_to_witness(&point.0), FC::fe_to_witness(&point.1));

        let x_assigned = self.field_chip.load_private(ctx, x);
        let y_assigned = self.field_chip.load_private(ctx, y);

        EcPoint::construct(x_assigned, y_assigned)
    }

    /// Does not constrain witness to lie on curve
    pub fn assign_point<'v, C>(
        &self,
        ctx: &mut Context<'_, F>,
        g: Value<C>,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt<Base = FC::FieldType>,
    {
        let (x, y) = g.map(|g| g.into_coordinates()).unzip();
        self.load_private(ctx, (x, y))
    }

    pub fn assign_constant_point<'v, C>(
        &self,
        ctx: &mut Context<'_, F>,
        g: C,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt<Base = FC::FieldType>,
    {
        let (x, y) = g.into_coordinates();
        let [x, y] = [x, y].map(FC::fe_to_constant);
        let x = self.field_chip.load_constant(ctx, x);
        let y = self.field_chip.load_constant(ctx, y);

        EcPoint::construct(x, y)
    }

    pub fn load_random_point<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt<Base = FC::FieldType>,
    {
        load_random_point::<F, FC, C>(self.field_chip(), ctx)
    }

    pub fn assert_is_on_curve<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
    ) where
        C: CurveAffine<Base = FC::FieldType>,
    {
        is_on_curve::<F, FC, C>(&self.field_chip, ctx, P)
    }

    pub fn is_on_curve_or_infinity<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
    ) -> AssignedValue<'v, F>
    where
        C: CurveAffine<Base = FC::FieldType>,
        C::Base: ff::PrimeField,
    {
        let lhs = self.field_chip.mul_no_carry(ctx, &P.y, &P.y);
        let mut rhs = self.field_chip.mul(ctx, &P.x, &P.x);
        rhs = self.field_chip.mul_no_carry(ctx, &rhs, &P.x);

        let b = FC::fe_to_constant(C::b());
        rhs = self.field_chip.add_constant_no_carry(ctx, &rhs, b);
        let mut diff = self.field_chip.sub_no_carry(ctx, &lhs, &rhs);
        diff = self.field_chip.carry_mod(ctx, &diff);

        let is_on_curve = self.field_chip.is_zero(ctx, &diff);

        let x_is_zero = self.field_chip.is_zero(ctx, &P.x);
        let y_is_zero = self.field_chip.is_zero(ctx, &P.y);

        self.field_chip.range().gate().or_and(
            ctx,
            Existing(&is_on_curve),
            Existing(&x_is_zero),
            Existing(&y_is_zero),
        )
    }

    pub fn negate<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
    ) -> EcPoint<F, FC::FieldPoint<'v>> {
        EcPoint::construct(P.x.clone(), self.field_chip.negate(ctx, &P.y))
    }

    /// Assumes that P.x != Q.x
    /// If `is_strict == true`, then actually constrains that `P.x != Q.x`
    pub fn add_unequal<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        Q: &EcPoint<F, FC::FieldPoint<'v>>,
        is_strict: bool,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        FC::FieldType: From<u64>,
    {
        ec_add_unequal(&self.field_chip, ctx, P, Q, is_strict)
    }

    /// Assumes that P.x != Q.x
    /// Otherwise will panic
    pub fn sub_unequal<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        Q: &EcPoint<F, FC::FieldPoint<'v>>,
        is_strict: bool,
    ) -> EcPoint<F, FC::FieldPoint<'v>> {
        ec_sub_unequal(&self.field_chip, ctx, P, Q, is_strict)
    }

    pub fn double<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
    ) -> EcPoint<F, FC::FieldPoint<'v>> {
        ec_double(&self.field_chip, ctx, P)
    }

    pub fn is_equal<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        Q: &EcPoint<F, FC::FieldPoint<'v>>,
    ) -> AssignedValue<'v, F> {
        // TODO: optimize
        let x_is_equal = self.field_chip.is_equal(ctx, &P.x, &Q.x);
        let y_is_equal = self.field_chip.is_equal(ctx, &P.y, &Q.y);
        self.field_chip.range().gate().and(ctx, Existing(&x_is_equal), Existing(&y_is_equal))
    }

    pub fn assert_equal<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        Q: &EcPoint<F, FC::FieldPoint<'v>>,
    ) {
        self.field_chip.assert_equal(ctx, &P.x, &Q.x);
        self.field_chip.assert_equal(ctx, &P.y, &Q.y);
    }

    pub fn sum<'b, 'v: 'b, C>(
        &self,
        ctx: &mut Context<'v, F>,
        points: impl Iterator<Item = &'b EcPoint<F, FC::FieldPoint<'v>>>,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt<Base = FC::FieldType>,
        FC::FieldPoint<'v>: 'b,
        FC::FieldType: From<u64>,
    {
        let rand_point = self.load_random_point::<C>(ctx);
        self.field_chip.enforce_less_than(ctx, rand_point.x());
        let mut acc = rand_point.clone();
        for point in points {
            self.field_chip.enforce_less_than(ctx, point.x());
            acc = self.add_unequal(ctx, &acc, point, true);
            self.field_chip.enforce_less_than(ctx, acc.x());
        }
        self.sub_unequal(ctx, &acc, &rand_point, true)
    }
}

impl<F: PrimeField, FC: FieldChip<F>> EccChip<F, FC>
where
    for<'v> FC: Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
{
    pub fn select<'v>(
        &self,
        ctx: &mut Context<'_, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        Q: &EcPoint<F, FC::FieldPoint<'v>>,
        condition: &AssignedValue<'v, F>,
    ) -> EcPoint<F, FC::FieldPoint<'v>> {
        ec_select(&self.field_chip, ctx, P, Q, condition)
    }

    pub fn scalar_mult<'v>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &EcPoint<F, FC::FieldPoint<'v>>,
        scalar: &Vec<AssignedValue<'v, F>>,
        max_bits: usize,
        window_bits: usize,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        FC::FieldType: From<u64>,
    {
        scalar_multiply::<F, FC>(&self.field_chip, ctx, P, scalar, max_bits, window_bits)
    }

    // TODO: put a check in place that scalar is < modulus of C::Scalar
    pub fn variable_base_msm<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
        P: &[EcPoint<F, FC::FieldPoint<'v>>],
        scalars: &[Vec<AssignedValue<'v, F>>],
        max_bits: usize,
        window_bits: usize,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt<Base = FC::FieldType>,
        C::Base: ff::PrimeField,
    {
        #[cfg(feature = "display")]
        println!("computing length {} MSM", P.len());

        if P.len() <= 25 {
            multi_scalar_multiply::<F, FC, C>(
                &self.field_chip,
                ctx,
                P,
                scalars,
                max_bits,
                window_bits,
            )
        } else {
            /*let mut radix = (f64::from((max_bits * scalars[0].len()) as u32)
                / f64::from(P.len() as u32))
            .sqrt()
            .floor() as usize;
            if radix == 0 {
                radix = 1;
            }*/
            let radix = 1;
            pippenger::multi_exp::<F, FC, C>(
                &self.field_chip,
                ctx,
                P,
                scalars,
                max_bits,
                radix,
                window_bits,
            )
        }
    }
}

impl<F: PrimeField, FC: PrimeFieldChip<F>> EccChip<F, FC>
where
    FC::FieldType: PrimeField,
{
    // TODO: put a check in place that scalar is < modulus of C::Scalar
    pub fn fixed_base_scalar_mult<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
        point: &C,
        scalar: &[AssignedValue<'v, F>],
        max_bits: usize,
        window_bits: usize,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt,
        FC: PrimeFieldChip<F, FieldType = C::Base, FieldPoint<'v> = CRTInteger<'v, F>>
            + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
    {
        fixed_base::scalar_multiply::<F, _, _>(
            &self.field_chip,
            ctx,
            point,
            scalar,
            max_bits,
            window_bits,
        )
    }

    /// `radix = 0` means auto-calculate
    ///
    /// `clump_factor = 0` means auto-calculate
    ///
    /// The user should filter out base points that are identity beforehand; we do not separately do this here
    pub fn fixed_base_msm<'v, C>(
        &self,
        ctx: &mut Context<'v, F>,
        points: &[C],
        scalars: &[Vec<AssignedValue<'v, F>>],
        max_scalar_bits_per_cell: usize,
        _radix: usize,
        clump_factor: usize,
    ) -> EcPoint<F, FC::FieldPoint<'v>>
    where
        C: CurveAffineExt,
        FC: PrimeFieldChip<F, FieldType = C::Base, FieldPoint<'v> = CRTInteger<'v, F>>
            + Selectable<F, Point<'v> = FC::FieldPoint<'v>>,
    {
        assert_eq!(points.len(), scalars.len());
        #[cfg(feature = "display")]
        println!("computing length {} fixed base msm", points.len());

        fixed_base::msm(self, ctx, points, scalars, max_scalar_bits_per_cell, clump_factor)

        // Empirically does not seem like pippenger is any better for fixed base msm right now, because of the cost of `select_by_indicator`
        // Cell usage becomes around comparable when `points.len() > 100`, and `clump_factor` should always be 4
        /*
        let radix = if radix == 0 {
            // auto calculate
            (f64::from(FC::FieldType::NUM_BITS) / f64::from(points.len() as u32)).sqrt().ceil()
                as usize
        } else {
            radix
        };
        assert!(radix > 0);

        fixed_base_pippenger::multi_exp::<F, FC, C>(
            self.field_chip,
            ctx,
            points,
            scalars,
            max_scalar_bits_per_cell,
            radix,
            clump_factor,
        )
        */
    }
}

#[cfg(test)]
pub(crate) mod tests;
