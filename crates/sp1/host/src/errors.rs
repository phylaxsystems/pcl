
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProofGenError {
    #[error("Invalid inputs")]
    InvalidInputs,
    #[error("Bincode serialization error")]
    Bincode(#[from] bincode::Error),
    #[error("Proof verification error")]
    ProofVerification(#[from] sp1_sdk::SP1VerificationError),
    #[error("Proof generation error: {0}")]
    ProofGeneration(String)
}
