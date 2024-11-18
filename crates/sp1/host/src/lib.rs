use sp1_sdk::{utils, HashableKey, ProverClient, SP1ProofWithPublicValues, SP1Stdin};
use tracing::info;

pub mod config;
pub mod errors;

/// The ELF we want to execute inside the zkVM.
const ELF: &[u8] = include_bytes!("../../../../elf/riscv32im-succinct-zkvm-elf");

pub async fn gen_por(inputs: config::PoRUserInputs) -> Result<(), errors::ProofGenError> {
    // Feed the sketch into the client.
    let input_bytes = bincode::serialize(&inputs)?;
    let mut stdin = SP1Stdin::new();
    stdin.write(&input_bytes);
    // Create a `ProverClient`.
    let client = ProverClient::new();

    // Generate the proof for the given program and input.
    let (pk, vk) = client.setup(ELF);
    let proof = client.prove(&pk, stdin).plonk().run().map_err(|err|errors::ProofGenError::ProofGeneration(err.to_string()))?;
    info!("PoR Generated");

    // Verify proof and public values.
    client.verify(&proof, &vk)?;
    info!("PoR Verified");
    Ok(())
}
