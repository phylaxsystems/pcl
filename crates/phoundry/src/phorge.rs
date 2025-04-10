use forge::cmd::{build::BuildArgs, flatten::FlattenArgs, test::TestArgs};
use pcl_common::args::CliArgs;
use std::{
    env,
    path::PathBuf,
    process::{Command, Output, Stdio}, str::FromStr,
};
use clap::Parser;

use crate::{error::PhoundryError};

#[derive(clap::Parser)]
pub struct PhorgeTest {
    #[clap(flatten)]
    pub args: TestArgs,
}

#[derive(clap::Parser)]
pub struct PhorgeBuild {
    pub root: PathBuf,
    pub assertion_file: PathBuf
}

impl PhorgeBuild {
    pub fn run(self) -> Result<(), PhoundryError> {
        let args = BuildArgs {
            paths: Some(vec![self.assertion_file]),
            build: cli::opts::build::BuildOpts {
                root: Some(self.root),
                ..Default::default()
            },
            ..Default::default()
        };
        let res = args.run().map_err(|e| PhoundryError::from(e))?;
        Ok(())
    }
}
#[derive(clap::Parser)]
pub struct PhorgeFlatten {
    pub assertion_file: PathBuf
}

impl PhorgeFlatten {
    pub fn run(self) -> Result<(), PhoundryError> {
        let temp_dir = tempdir::TempDir::new("flatten").expect("It should be able to create a temp dir");
        let file_path = temp_dir.path().join("flattened.sol");
        let args = FlattenArgs {
            paths: Some(vec![self.assertion_file]),
            output: Some(file_path.to_str().unwrap().to_string()),
            ..Default::default()
        };
        args.run().map_err(|e| PhoundryError::from(e))?;
        let flattened = std::fs::read_to_string(file_path)?;
        Ok(())
    }
}

impl PhorgeTest {
   pub async fn run(self) -> Result<(), PhoundryError> {
        let result = self.args.run().await?;
        Ok(())
   }

}
    // fn build_phorge_args(
    //     &self,
    //     cli_args: &CliArgs,
    //     foundry_config: &foundry_config::Config,
    // ) -> Result<Vec<String>, PhoundryError> {
    //     if let Some(ref root_dir) = cli_args.root_dir {
    //         args.push("--root".to_string());
    //         args.push(root_dir.to_str().unwrap().to_string());
    //     }
    //     // Check if profile exists in config, then enable it
    //     if let Some(ref profile) = cli_args.foundry_profile {
    //         let exists = foundry_config.profiles.iter().find(|p| p.to_string() == profile.to_string());
    //         if exists.is_none() {
    //             return Err(PhoundryError::InvalidFoundryProfile(profile.to_string(), cli_args.root_dir()));
    //         }
    //         env::set_var("FOUNDRY_PROFILE", profile);
    //     }

    //     // Only valid for the context of this binary execution
    //     env::set_var(
    //         "FOUNDRY_SRC",
    //         cli_args.assertions_src().as_os_str().to_str().unwrap(),
    //     );

    //     env::set_var(
    //         "FOUNDRY_TEST",
    //         cli_args.assertions_test().as_os_str().to_str().unwrap(),
    //     );
    //     todo!()
    // }