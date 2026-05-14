//! Shared helpers for deriving the constructor ABI signature and ABI-encoding
//! constructor arguments from a compiled `JsonAbi`.
//!
//! Used by `apply` (to forward the typed signature to the dApp) and `verify`
//! (to build deployment bytecode for local source verification). Centralising
//! the logic ensures both paths produce identical signatures and encoded bytes
//! given the same ABI + args.

use alloy_dyn_abi::{
    DynSolValue,
    JsonAbiExt,
    Specifier,
};
use alloy_json_abi::{
    JsonAbi,
    Param,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConstructorAbiError {
    #[error("expected {expected} constructor argument{}, got {actual}", if *expected == 1 { "" } else { "s" })]
    ArgCountMismatch { expected: usize, actual: usize },

    #[error("unsupported constructor type '{ty}': {source}")]
    UnsupportedType {
        ty: String,
        #[source]
        source: alloy_dyn_abi::Error,
    },

    #[error("failed to parse constructor arg '{arg}' as {ty}: {source}")]
    CoerceFailure {
        arg: String,
        ty: String,
        #[source]
        source: alloy_dyn_abi::Error,
    },

    #[error("constructor args ABI encode failed: {0}")]
    EncodeFailure(String),
}

/// Constructor input params, or an empty slice if the ABI has no constructor.
fn constructor_inputs(abi: &JsonAbi) -> &[Param] {
    abi.constructor
        .as_ref()
        .map_or(&[], |constructor| constructor.inputs.as_slice())
}

/// Build the canonical Solidity constructor signature, e.g. `constructor(address,uint256)`.
/// Uses `selector_type()` so structs/tuples render with their full type signature.
pub fn build_signature(abi: &JsonAbi, args: &[String]) -> Result<String, ConstructorAbiError> {
    let inputs = constructor_inputs(abi);
    if inputs.len() != args.len() {
        return Err(ConstructorAbiError::ArgCountMismatch {
            expected: inputs.len(),
            actual: args.len(),
        });
    }

    let types: Vec<_> = inputs
        .iter()
        .map(|p| p.selector_type().into_owned())
        .collect();
    Ok(format!("constructor({})", types.join(",")))
}

/// ABI-encode constructor args per their declared types.
/// Returns an empty `Vec` when the ABI has no constructor and no args were provided.
pub fn encode_args(abi: &JsonAbi, args: &[String]) -> Result<Vec<u8>, ConstructorAbiError> {
    let inputs = constructor_inputs(abi);
    if inputs.len() != args.len() {
        return Err(ConstructorAbiError::ArgCountMismatch {
            expected: inputs.len(),
            actual: args.len(),
        });
    }

    let Some(constructor) = abi.constructor.as_ref() else {
        return Ok(Vec::new());
    };

    let values: Vec<DynSolValue> = inputs
        .iter()
        .zip(args.iter())
        .map(|(param, arg)| {
            let sol_type = param.resolve().map_err(|e| {
                ConstructorAbiError::UnsupportedType {
                    ty: param.selector_type().into_owned(),
                    source: e,
                }
            })?;
            sol_type.coerce_str(arg).map_err(|e| {
                ConstructorAbiError::CoerceFailure {
                    arg: arg.clone(),
                    ty: param.selector_type().into_owned(),
                    source: e,
                }
            })
        })
        .collect::<Result<_, _>>()?;

    constructor
        .abi_encode_input(&values)
        .map_err(|e| ConstructorAbiError::EncodeFailure(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_json_abi::{
        Constructor,
        Param,
        StateMutability,
    };
    use alloy_primitives::hex;

    fn make_param(ty: &str, name: &str) -> Param {
        Param {
            ty: ty.to_string(),
            name: name.to_string(),
            components: vec![],
            internal_type: None,
        }
    }

    fn make_abi(inputs: Vec<Param>) -> JsonAbi {
        JsonAbi {
            constructor: Some(Constructor {
                inputs,
                state_mutability: StateMutability::NonPayable,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn no_constructor_no_args_defaults_to_empty() {
        let abi = JsonAbi::default();
        assert_eq!(build_signature(&abi, &[]).unwrap(), "constructor()");
        assert_eq!(encode_args(&abi, &[]).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn no_constructor_with_args_rejects_count_mismatch() {
        let abi = JsonAbi::default();
        let err = build_signature(&abi, &["42".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            ConstructorAbiError::ArgCountMismatch {
                expected: 0,
                actual: 1
            }
        ));
        let err = encode_args(&abi, &["42".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            ConstructorAbiError::ArgCountMismatch {
                expected: 0,
                actual: 1
            }
        ));
    }

    #[test]
    fn wrong_arg_count_rejected() {
        let abi = make_abi(vec![make_param("uint256", "x")]);
        let err = build_signature(&abi, &[]).unwrap_err();
        assert!(matches!(
            err,
            ConstructorAbiError::ArgCountMismatch {
                expected: 1,
                actual: 0
            }
        ));

        let err = encode_args(&abi, &["1".to_string(), "2".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            ConstructorAbiError::ArgCountMismatch {
                expected: 1,
                actual: 2
            }
        ));
    }

    #[test]
    fn preserves_address_type() {
        let abi = make_abi(vec![make_param("address", "_owner")]);
        let args = vec!["0xF31b02F47596AcC7328E9fb04aFc52Fe91Da6071".to_string()];

        assert_eq!(
            build_signature(&abi, &args).unwrap(),
            "constructor(address)"
        );
        assert_eq!(
            hex::encode_prefixed(encode_args(&abi, &args).unwrap()),
            "0x000000000000000000000000f31b02f47596acc7328e9fb04afc52fe91da6071"
        );
    }

    #[test]
    fn handles_multiple_args() {
        let abi = make_abi(vec![
            make_param("address", "vault"),
            make_param("uint256", "threshold"),
        ]);
        let args = vec![
            "0xF31b02F47596AcC7328E9fb04aFc52Fe91Da6071".to_string(),
            "42".to_string(),
        ];

        assert_eq!(
            build_signature(&abi, &args).unwrap(),
            "constructor(address,uint256)"
        );
        assert_eq!(
            hex::encode_prefixed(encode_args(&abi, &args).unwrap()),
            concat!(
                "0x000000000000000000000000f31b02f47596acc7328e9fb04afc52fe91da6071",
                "000000000000000000000000000000000000000000000000000000000000002a",
            )
        );
    }

    #[test]
    fn rejects_unparseable_value() {
        let abi = make_abi(vec![make_param("uint256", "x")]);
        let err = encode_args(&abi, &["not_a_number".to_string()]).unwrap_err();
        assert!(
            matches!(err, ConstructorAbiError::CoerceFailure { .. }),
            "got: {err}"
        );
    }
}
