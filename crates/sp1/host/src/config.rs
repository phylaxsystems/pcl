use clap::Parser;
use alloy_primitives::{Address, BlockHash, BlockNumber, Bytes};
use serde::Serialize;
#[derive(Debug, Parser, Serialize)]
pub struct PoRUserInputs {
    /// The Ethereum address of the assertion adopter that will receive the proof
    #[arg(short = 'a', long, help = "Ethereum address of the assertion adopter")]
    assertion_adopter_address: Address,

    /// The Ethereum address of the PoR submitter that will submit the proof
    #[arg(short = 's', long, help = "Ethereum address of the PoR submitter")]
    por_submitter: Address,

    /// The block hash to generate the proof for
    #[arg(short = 'h', long, help = "Block hash to generate proof for")]
    block_hash: BlockHash,

    /// The block number to generate the proof for
    #[arg(short = 'n', long, help = "Block number to generate proof for")]
    block_number: BlockNumber,

    /// The assertion bytes containing the proof data
    #[arg(short = 'b', long, help = "Assertion bytes containing proof data")]
    assertion: Bytes,
}


pub struct PoRInputs {
    pub user_inputs: PoRUserInputs,
}
