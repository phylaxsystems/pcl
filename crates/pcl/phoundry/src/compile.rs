use forge::cmd::build::BuildArgs;
use foundry_cli::{
    opts::BuildOpts,
    utils::LoadConfig,
};
use foundry_common::compile::ProjectCompiler;
use foundry_compilers::ProjectCompileOutput;
use foundry_config::Config;

use crate::error::PhoundryError;

/// Compiles the project and returns the compilation output.
pub fn compile(build_opts: BuildOpts) -> Result<ProjectCompileOutput, Box<PhoundryError>> {
    Ok(compile_with_config(build_opts)?.0)
}

/// Compiles with the loaded config and returns that config.
///
/// Flattening must reuse this config. Loading `foundry.toml` again can pick up
/// `libs` or `include_paths` that were not part of this compilation.
pub(crate) fn compile_with_config(
    build_opts: BuildOpts,
) -> Result<(ProjectCompileOutput, Config), Box<PhoundryError>> {
    let build_cmd = BuildArgs {
        build: build_opts,
        ..Default::default()
    };

    let config = build_cmd.load_config()?;

    let project = config.project().map_err(PhoundryError::SolcError)?;
    let contracts = project.sources_path();

    match std::fs::read_dir(contracts) {
        Ok(mut files) => {
            // Check if the directory is empty
            if files.next().is_none() {
                return Err(Box::new(PhoundryError::NoSourceFilesFound));
            }
        }
        Err(_) => {
            return Err(Box::new(PhoundryError::DirectoryNotFound(
                contracts.to_path_buf(),
            )));
        }
    }

    let compiler = ProjectCompiler::new()
        .dynamic_test_linking(config.dynamic_test_linking)
        .print_names(build_cmd.names)
        .print_sizes(build_cmd.sizes)
        .ignore_eip_3860(build_cmd.ignore_eip_3860)
        .bail(true)
        .quiet(true);

    let res = compiler
        .compile(&project)
        .map_err(PhoundryError::CompilationError)?;
    Ok((res, config))
}
