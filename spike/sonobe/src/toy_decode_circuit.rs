// ToyDecodeFCircuit — folding-step circuit modeling the M2 toy codec's
// decode kernel.
//
// At M1b's stub level, the in-circuit decoder mirrors what
// `crates/toy-codec` does for QP=0: take a 16-bit signed coefficient,
// clamp it to [0, 255], emit the resulting u8.
//
// Step shape:
//   state_len = 2
//   z_i = [coefficient_sum, clamped_sum]
//   ext_i = one i16 coefficient (passed as u16 two's-complement)
//
//   Compute clamped = clamp(ext_i)
//          z_{i+1} = [z_i[0] + ext_i, z_i[1] + clamped]
//
// One fold step per coefficient. For a 16x16 4:2:0 fixture (smallest
// valid frame), that's 256 Y + 64 U + 64 V = 384 fold steps. Cheap
// enough to bench end-to-end on CPU.
//
// Why clamp specifically: the existing toy-codec stub at QP=0 is a
// pixel-value passthrough whose only constraint is `[0,255]`. The
// "real" milestone-2 step circuit will dequantize + inverse-WHT/IDCT +
// clamp. This file provides the scaffold and the clamp gadget; replace
// generate_step_constraints with the full kernel once the M2 codec
// stops being a stub.
//
// Arithmetic notes (clamp via 16-bit decomposition):
//   sign_bit       = bit 15 of ext_i (1 iff negative two's-complement)
//   upper_nonzero  = OR(bits 8..15)  (true iff |value| >= 256 unsigned,
//                                     i.e. saturated when positive)
//   low_byte       = sum_{j=0..8} bits[j] * 2^j
//
//   in_range  = !sign_bit AND !upper_nonzero
//   negative  =  sign_bit
//   saturated = !sign_bit AND  upper_nonzero
//
//   clamped =  in_range  * low_byte
//           +  negative  * 0
//           +  saturated * 255

use ark_ff::PrimeField;
use ark_r1cs_std::{
    alloc::AllocVar,
    boolean::Boolean,
    fields::fp::FpVar,
    prelude::*,
    uint16::UInt16,
};
use ark_relations::gr1cs::{ConstraintSystemRef, Namespace, SynthesisError};

use folding_schemes::frontend::FCircuit;
use folding_schemes::Error;

#[derive(Clone, Copy, Debug, Default)]
pub struct ToyDecodeExt {
    /// One quantized coefficient, packed as u16 two's-complement of i16.
    pub coeff_u16: u16,
}

#[derive(Clone, Debug)]
pub struct ToyDecodeExtVar<F: PrimeField> {
    pub coeff: UInt16<F>,
}

impl<F: PrimeField> AllocVar<ToyDecodeExt, F> for ToyDecodeExtVar<F> {
    fn new_variable<T: core::borrow::Borrow<ToyDecodeExt>>(
        cs: impl Into<Namespace<F>>,
        f: impl FnOnce() -> Result<T, SynthesisError>,
        mode: ark_r1cs_std::alloc::AllocationMode,
    ) -> Result<Self, SynthesisError> {
        let ns = cs.into();
        let cs = ns.cs();
        let value = f().map(|v| *v.borrow()).unwrap_or_default();
        let coeff = UInt16::new_variable(cs, || Ok(value.coeff_u16), mode)?;
        Ok(Self { coeff })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ToyDecodeFCircuit<F: PrimeField> {
    _f: core::marker::PhantomData<F>,
}

impl<F: PrimeField> FCircuit<F> for ToyDecodeFCircuit<F> {
    type Params = ();
    type ExternalInputs = ToyDecodeExt;
    type ExternalInputsVar = ToyDecodeExtVar<F>;

    fn new(_params: Self::Params) -> Result<Self, Error> {
        Ok(Self {
            _f: core::marker::PhantomData,
        })
    }

    fn state_len(&self) -> usize {
        2
    }

    fn generate_step_constraints(
        &self,
        _cs: ConstraintSystemRef<F>,
        _i: usize,
        z_i: Vec<FpVar<F>>,
        external_inputs: Self::ExternalInputsVar,
    ) -> Result<Vec<FpVar<F>>, SynthesisError> {
        let bits = external_inputs.coeff.to_bits_le()?; // [bit_0, ..., bit_15]
        debug_assert_eq!(bits.len(), 16);

        // Reconstruct the unsigned 16-bit value as a field element so we
        // can fold it into the coefficient_sum running tally.
        let coeff_field = bits_to_fp(&bits);

        // Low byte = sum_{j=0..8} bit_j * 2^j  (the value modulo 256).
        let low_byte = bits_to_fp(&bits[0..8]);

        // Sign bit (two's complement) = bit 15.
        let sign_bit = bits[15].clone();

        // upper_nonzero = OR of bits 8..15. The forked r1cs-std exposes
        // Boolean::kary_or for n-ary disjunctions.
        let upper_nonzero = Boolean::kary_or(&bits[8..15])?;

        // The fork exposes !x via the unary Not operator; pairwise AND
        // via kary_and on a 2-element slice.
        let not_sign = !&sign_bit;
        let not_upper = !&upper_nonzero;

        let in_range = Boolean::kary_and(&[not_sign.clone(), not_upper])?;
        let saturated = Boolean::kary_and(&[not_sign, upper_nonzero])?;
        // negative = sign_bit (definitionally; bit15=1 means negative i16)

        let two_fifty_five = FpVar::<F>::constant(F::from(255u32));

        // clamped = in_range * low_byte + saturated * 255 + sign_bit * 0
        // (the sign_bit branch is omitted because it contributes 0).
        let in_range_term = FpVar::from(in_range) * &low_byte;
        let saturated_term = FpVar::from(saturated) * &two_fifty_five;
        let clamped = in_range_term + saturated_term;

        // z_{i+1} = [coefficient_sum + coeff, clamped_sum + clamped]
        let new_coeff_sum = &z_i[0] + &coeff_field;
        let new_clamped_sum = &z_i[1] + &clamped;
        Ok(vec![new_coeff_sum, new_clamped_sum])
    }
}

fn bits_to_fp<F: PrimeField>(bits: &[Boolean<F>]) -> FpVar<F> {
    // sum_{j} bit_j * 2^j  in field arithmetic
    let mut acc = FpVar::<F>::zero();
    let mut weight = F::one();
    for b in bits {
        acc += FpVar::from(b.clone()) * FpVar::<F>::constant(weight);
        weight.double_in_place();
    }
    acc
}

