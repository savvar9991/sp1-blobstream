#![no_main]
sp1_zkvm::entrypoint!(main);

use alloy::primitives::B256;
use alloy::sol;
use alloy::sol_types::SolType;
use primitives::bool_array_to_uint256;
use primitives::get_header_update_verdict;
use primitives::types::ProofInputs;
use primitives::types::ProofOutputs;
use sha2::Sha256;
use std::collections::HashSet;
use tendermint::{block::Header, merkle::simple_hash_from_byte_vectors};
use tendermint_light_client_verifier::Verdict;
type DataRootTuple = sol! {
    tuple(uint64, bytes32)
};

/// Compute the data commitment for the given headers.
fn compute_data_commitment(headers: &[Header]) -> [u8; 32] {
    let mut encoded_data_root_tuples: Vec<Vec<u8>> = Vec::new();
    for i in 1..headers.len() {
        let prev_header = &headers[i - 1];
        let curr_header = &headers[i];
        // Checks that chain of headers is well-formed.
        if prev_header.hash() != curr_header.last_block_id.unwrap().hash {
            panic!("invalid header");
        }

        let data_hash: [u8; 32] = prev_header
            .data_hash
            .unwrap()
            .as_bytes()
            .try_into()
            .unwrap();

        let data_root_tuple = DataRootTuple::abi_encode(&(prev_header.height.value(), data_hash));
        encoded_data_root_tuples.push(data_root_tuple);
    }

    simple_hash_from_byte_vectors::<Sha256>(&encoded_data_root_tuples)
}

fn main() {
    let proof_inputs_vec = sp1_zkvm::io::read_vec();
    let proof_inputs = serde_cbor::from_slice(&proof_inputs_vec).unwrap();

    let ProofInputs {
        trusted_block_height,
        target_block_height,
        trusted_light_block,
        target_light_block,
        headers,
    } = proof_inputs;

    let verdict = get_header_update_verdict(&trusted_light_block, &target_light_block);

    match verdict {
        Verdict::Success => {
            println!("success");
        }
        v => panic!("Could not verify updating to target_block, error: {:?}", v),
    }

    let mut all_headers = Vec::new();
    all_headers.push(trusted_light_block.signed_header.header.clone());
    all_headers.extend(headers);
    all_headers.push(target_light_block.signed_header.header.clone());

    let data_commitment = B256::from_slice(&compute_data_commitment(&all_headers));

    // Now that we have verified our proof, we commit the header hashes to the zkVM to expose
    // them as public values.
    let trusted_header_hash =
        B256::from_slice(trusted_light_block.signed_header.header.hash().as_bytes());
    let target_header_hash =
        B256::from_slice(target_light_block.signed_header.header.hash().as_bytes());

    // Construct a bitmap of the intersection of the validators that signed off on the trusted and
    // target header. Use the order of the validators from the trusted header. Used to equivocate
    // slashing in the case that validators are malicious. 256 is chosen as the maximum number of
    // validators as it is unlikely that Celestia has >256 validators.
    let mut validators = HashSet::new();
    for i in 0..trusted_light_block.signed_header.commit.signatures.len() {
        for j in 0..target_light_block.signed_header.commit.signatures.len() {
            let trusted_sig = &trusted_light_block.signed_header.commit.signatures[i];
            let target_sig = &target_light_block.signed_header.commit.signatures[j];

            if trusted_sig.is_commit()
                && target_sig.is_commit()
                && trusted_sig.validator_address() == target_sig.validator_address()
            {
                validators.insert(trusted_sig.validator_address().unwrap());
            }
        }
    }

    let mut validator_bitmap = [false; 256];
    for (i, validator) in trusted_light_block
        .validators
        .validators()
        .iter()
        .enumerate()
    {
        if validators.contains(&validator.address) {
            validator_bitmap[i] = true;
        }
    }

    // Convert the validator bitmap to a uint256.
    let validator_bitmap_u256 = bool_array_to_uint256(validator_bitmap);

    // ABI-Encode the proof outputs.
    let proof_outputs = ProofOutputs::abi_encode(&(
        trusted_header_hash,
        target_header_hash,
        data_commitment,
        trusted_block_height,
        target_block_height,
        validator_bitmap_u256,
    ));
    sp1_zkvm::io::commit_slice(&proof_outputs);
}
