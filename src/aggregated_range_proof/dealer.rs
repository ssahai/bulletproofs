use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use generators::GeneratorsView;
use inner_product_proof;
use proof_transcript::ProofTranscript;
use util;

use super::messages::*;

/// Dealer is an entry-point API for setting up a dealer
pub struct Dealer {}

impl Dealer {
    /// Creates a new dealer coordinating `m` parties proving `n`-bit ranges.
    pub fn new<'a>(
        n: usize,
        m: usize,
        transcript: &'a mut ProofTranscript,
    ) -> Result<DealerAwaitingValueCommitments<'a>, &'static str> {
        if !n.is_power_of_two() || n > 64 {
            return Err("n is not valid: must be a power of 2, and less than or equal to 64");
        }
        if !m.is_power_of_two() {
            return Err("m is not valid: must be a power of 2");
        }
        transcript.commit_u64(n as u64);
        transcript.commit_u64(m as u64);
        Ok(DealerAwaitingValueCommitments { n, m, transcript })
    }
}

/// When the dealer is initialized, it only knows the size of the set.
pub struct DealerAwaitingValueCommitments<'a> {
    n: usize,
    m: usize,
    transcript: &'a mut ProofTranscript,
}

impl<'a> DealerAwaitingValueCommitments<'a> {
    /// Combines commitments and computes challenge variables.
    pub fn receive_value_commitments(
        self,
        value_commitments: &Vec<ValueCommitment>,
    ) -> Result<(DealerAwaitingPolyCommitments<'a>, ValueChallenge), &'static str> {
        if self.m != value_commitments.len() {
            return Err("Length of value commitments doesn't match expected length m");
        }

        let mut A = RistrettoPoint::identity();
        let mut S = RistrettoPoint::identity();

        for commitment in value_commitments.iter() {
            // Commit each V individually
            self.transcript.commit(commitment.V.compress().as_bytes());

            // Commit sums of As and Ss.
            A += commitment.A;
            S += commitment.S;
        }

        self.transcript.commit(A.compress().as_bytes());
        self.transcript.commit(S.compress().as_bytes());

        let y = self.transcript.challenge_scalar();
        let z = self.transcript.challenge_scalar();
        let value_challenge = ValueChallenge { y, z };

        Ok((
            DealerAwaitingPolyCommitments {
                n: self.n,
                m: self.m,
                transcript: self.transcript,
                value_challenge: value_challenge.clone(),
            },
            value_challenge,
        ))
    }
}

pub struct DealerAwaitingPolyCommitments<'a> {
    n: usize,
    m: usize,
    transcript: &'a mut ProofTranscript,
    value_challenge: ValueChallenge,
}

impl<'a> DealerAwaitingPolyCommitments<'a> {
    pub fn receive_poly_commitments(
        self,
        poly_commitments: &Vec<PolyCommitment>,
    ) -> Result<(DealerAwaitingProofShares<'a>, PolyChallenge), &'static str> {
        if self.m != poly_commitments.len() {
            return Err("Length of poly commitments doesn't match expected length m");
        }

        // Commit sums of T1s and T2s.
        let mut T1 = RistrettoPoint::identity();
        let mut T2 = RistrettoPoint::identity();
        for commitment in poly_commitments.iter() {
            T1 += commitment.T_1;
            T2 += commitment.T_2;
        }
        self.transcript.commit(T1.compress().as_bytes());
        self.transcript.commit(T2.compress().as_bytes());

        let x = self.transcript.challenge_scalar();
        let poly_challenge = PolyChallenge { x };

        Ok((
            DealerAwaitingProofShares {
                n: self.n,
                m: self.m,
                transcript: self.transcript,
                value_challenge: self.value_challenge,
                poly_challenge: poly_challenge.clone(),
            },
            poly_challenge,
        ))
    }
}

pub struct DealerAwaitingProofShares<'a> {
    n: usize,
    m: usize,
    transcript: &'a mut ProofTranscript,
    value_challenge: ValueChallenge,
    poly_challenge: PolyChallenge,
}

impl<'a> DealerAwaitingProofShares<'a> {
    pub fn receive_shares(
        self,
        proof_shares: &Vec<ProofShare>,
        gen: &GeneratorsView,
    ) -> Result<(AggregatedProof, Vec<ProofShareVerifier>), &'static str> {
        if self.m != proof_shares.len() {
            return Err("Length of proof shares doesn't match expected length m");
        }

        let mut share_verifiers = Vec::new();
        for (j, proof_share) in proof_shares.iter().enumerate() {
            share_verifiers.push(ProofShareVerifier {
                proof_share: proof_share.clone(),
                n: self.n,
                j: j,
                value_challenge: self.value_challenge.clone(),
                poly_challenge: self.poly_challenge.clone(),
            });
        }

        let value_commitments = proof_shares
            .iter()
            .map(|ps| ps.value_commitment.V)
            .collect();
        let A = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |A, ps| {
                A + ps.value_commitment.A
            });
        let S = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |S, ps| {
                S + ps.value_commitment.S
            });
        let T_1 = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |T_1, ps| {
                T_1 + ps.poly_commitment.T_1
            });
        let T_2 = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |T_2, ps| {
                T_2 + ps.poly_commitment.T_2
            });
        let t = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.t_x);
        let t_x_blinding = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.t_x_blinding);
        let e_blinding = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.e_blinding);

        self.transcript.commit(t.as_bytes());
        self.transcript.commit(t_x_blinding.as_bytes());
        self.transcript.commit(e_blinding.as_bytes());

        // Get a challenge value to combine statements for the IPP
        let w = self.transcript.challenge_scalar();
        let Q = w * gen.pedersen_generators.B;

        let l_vec: Vec<Scalar> = proof_shares
            .iter()
            .flat_map(|ps| ps.l_vec.clone().into_iter())
            .collect();
        let r_vec: Vec<Scalar> = proof_shares
            .iter()
            .flat_map(|ps| ps.r_vec.clone().into_iter())
            .collect();
        let ipp_proof = inner_product_proof::InnerProductProof::create(
            self.transcript,
            &Q,
            util::exp_iter(self.value_challenge.y.invert()),
            gen.G.to_vec(),
            gen.H.to_vec(),
            l_vec.clone(),
            r_vec.clone(),
        );

        let aggregated_proof = AggregatedProof {
            n: self.n,
            value_commitments,
            A,
            S,
            T_1,
            T_2,
            t_x: t,
            t_x_blinding,
            e_blinding,
            ipp_proof,
        };

        Ok((aggregated_proof, share_verifiers))
    }
}
