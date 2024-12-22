use std::process::Command;

use anyhow::Result;
use vergen_gix::{
    BuildBuilder, CargoBuilder, Emitter, GixBuilder, RustcBuilder, SysinfoBuilder,
};

pub fn main() -> Result<()> {
    // Clone and build external repository
    println!("cargo:rerun-if-changed=build.rs");
    
    let repo_url = "https://github.com/phylaxsystems/phoundry";
    let repo_name = "phoundry";
    
    // Clone or update the repository to get latest version
    if !std::path::Path::new(repo_name).exists() {
        Command::new("git")
            .args(["clone", repo_url, repo_name])
            .status()
            .expect("Failed to clone repository");
    } else {
        // Fetch and update to latest
        Command::new("git")
            .current_dir(repo_name)
            .args(["pull", "origin", "main"])
            .status()
            .expect("Failed to pull from remote");

    }

    // Build the external project
    Command::new("cargo")
        .current_dir(repo_name)
        .args(["build", "--release"])
        .status()
        .expect("Failed to build external project");


    Emitter::default()
        .add_instructions(&BuildBuilder::all_build()?)?
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .add_instructions(&GixBuilder::all_git()?)?
        .add_instructions(&RustcBuilder::all_rustc()?)?
        .add_instructions(&SysinfoBuilder::all_sysinfo()?)?
        .emit()
}