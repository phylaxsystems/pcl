use forge::cmd::build::BuildArgs;
use foundry_cli::{
    opts::BuildOpts,
    utils::LoadConfig,
};
use foundry_common::compile::ProjectCompiler;
use foundry_compilers::ProjectCompileOutput;
use std::path::Path;

use crate::error::PhoundryError;

/// Compiles the project and returns the compilation output.
pub fn compile(build_opts: BuildOpts) -> Result<ProjectCompileOutput, Box<PhoundryError>> {
    compile_target(build_opts, None)
}

pub(crate) fn compile_target(
    build_opts: BuildOpts,
    target: Option<&Path>,
) -> Result<ProjectCompileOutput, Box<PhoundryError>> {
    let build_cmd = BuildArgs {
        build: build_opts,
        ..Default::default()
    };

    let config = build_cmd.load_config()?;

    let project = if target.is_some() {
        let mut project = config
            .ephemeral_project()
            .map_err(PhoundryError::SolcError)?;
        project.build_info = true;
        project
    } else {
        config.project().map_err(PhoundryError::SolcError)?
    };
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

    let mut compiler = ProjectCompiler::new()
        .dynamic_test_linking(config.dynamic_test_linking)
        .print_names(build_cmd.names)
        .print_sizes(build_cmd.sizes)
        .ignore_eip_3860(build_cmd.ignore_eip_3860)
        .bail(true)
        .quiet(true);
    if let Some(target) = target {
        compiler = compiler.files([target.to_path_buf()]);
    }

    let res = compiler
        .compile(&project)
        .map_err(PhoundryError::CompilationError)?;
    Ok(res)
}
