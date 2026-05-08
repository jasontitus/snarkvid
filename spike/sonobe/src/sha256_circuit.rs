// Sha256FCircuit — lifted directly from sonobe/examples/sha256.rs.
//
// Step semantics: state_len = 1; given z_i (one field element), compute
// z_{i+1} = first_field_element(SHA256(z_i.to_bytes_le())). The arkworks
// `Sha256Gadget` is what we use under the hood.
//
// To map this to "SHA-256 of N bytes" fixtures: number_of_steps =
// ceil(fixture_bytes / 32). Each step is one full SHA-256 evaluation
// (one block, since z_i fits in 32 bytes). Total constraints scale ~
// 25-30k per step (per arkworks Sha256Gadget). For our 1KB / 1MB / 10MB
// fixtures this gives 32 / 32768 / 327680 fold steps respectively — the
// 10MB row will not finish on a laptop and is documented as
// "GPU-or-cluster only" alongside the SP1/RISC0 numbers.
//
// This is the *stock* sonobe pattern. A "block-absorb" variant where each
// step pulls 64 input bytes via ExternalInputs is the right shape for
// the M2 codec workload but is a separate, custom step circuit. We
// document that in the milestone report and leave it as a follow-up.

use ark_crypto_primitives::crh::{
    sha256::constraints::{Sha256Gadget, UnitVar},
    CRHSchemeGadget,
};
use ark_ff::PrimeField;
use ark_r1cs_std::{
    convert::{ToBytesGadget, ToConstraintFieldGadget},
    fields::fp::FpVar,
};
use ark_relations::gr1cs::{ConstraintSystemRef, SynthesisError};
use core::marker::PhantomData;

use folding_schemes::frontend::FCircuit;
use folding_schemes::Error;

#[derive(Clone, Copy, Debug)]
pub struct Sha256FCircuit<F: PrimeField> {
    _f: PhantomData<F>,
}

impl<F: PrimeField> FCircuit<F> for Sha256FCircuit<F> {
    type Params = ();
    type ExternalInputs = ();
    type ExternalInputsVar = ();

    fn new(_params: Self::Params) -> Result<Self, Error> {
        Ok(Self { _f: PhantomData })
    }

    fn state_len(&self) -> usize {
        1
    }

    fn generate_step_constraints(
        &self,
        _cs: ConstraintSystemRef<F>,
        _i: usize,
        z_i: Vec<FpVar<F>>,
        _external_inputs: Self::ExternalInputsVar,
    ) -> Result<Vec<FpVar<F>>, SynthesisError> {
        let unit_var = UnitVar::default();
        let out_bytes = Sha256Gadget::evaluate(&unit_var, &z_i[0].to_bytes_le()?)?;
        let out = out_bytes.0.to_constraint_field()?;
        Ok(vec![out[0].clone()])
    }
}
