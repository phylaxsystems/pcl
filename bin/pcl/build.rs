use std::process::Command;

use anyhow::Result;
use vergen_gix::{
    BuildBuilder, CargoBuilder, Emitter, GixBuilder, RustcBuilder, SysinfoBuilder,
};

pub fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    let repo_url = "https://github.com/phylaxsystems/phoundry";
    let repo_name = "foundry";
    
    if !std::path::Path::new(repo_name).exists() {
        Command::new("git")
            .args(["clone", repo_url, repo_name])
            .status()
            .expect("Failed to clone repository");
    } else {
        Command::new("git")
            .current_dir(repo_name)
            .args(["pull", "origin", "master"])
            .status()
            .expect("Failed to pull from remote");
    }
    println!("cargo:rerun-if-changed={repo_name}");

    // Build forge 
    Command::new("cargo")
        .current_dir(repo_name)
        .args(["build", "--bin", "forge", "--release"])
        .status()
        .expect("Failed to build external project");

    // Rename forge and place it in the build directory
    let forge_build= format!("{repo_name}/target/release/forge");
    let out_dir = std::env::var("OUT_DIR").expect("Failed to get OUT_DIR");
    
    // Copy the binary to the output directory
    std::fs::copy(&forge_build, format!("{out_dir}/phorge"))
        .expect("Failed to copy binary to output directory");
    println!("cargo:rustc-env=PHOUNDRY_PATH={out_dir}/phorge");

    Emitter::default()
        .add_instructions(&BuildBuilder::all_build()?)?
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .add_instructions(&GixBuilder::all_git()?)?
        .add_instructions(&RustcBuilder::all_rustc()?)?
        .add_instructions(&SysinfoBuilder::all_sysinfo()?)?
        .emit()
}