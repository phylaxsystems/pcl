use std::{env, fs, path::Path, process::Command};

use anyhow::Result;
use vergen_gix::{BuildBuilder, CargoBuilder, Emitter, GixBuilder, RustcBuilder, SysinfoBuilder};

pub fn main() -> Result<()> {
    Emitter::default()
        .add_instructions(&BuildBuilder::all_build()?)?
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .add_instructions(&GixBuilder::all_git()?)?
        .add_instructions(&RustcBuilder::all_rustc()?)?
        .add_instructions(&SysinfoBuilder::all_sysinfo()?)?
        .emit()?;

    let profile = env::var("PROFILE").unwrap();
    println!("cargo:warning=Building in {} mode", profile);

    // Environment flags
    println!("cargo:rerun-if-env-changed=PCL_SKIP_UPDATE_PHOUNDRY");
    println!("cargo:rerun-if-env-changed=PCL_SKIP_BUILD_PHOUNDRY");

    let skip_build_phoundry = env::var("PCL_SKIP_BUILD_PHOUNDRY")
        .map(|val| val.to_lowercase() == "true")
        .unwrap_or(false);

    let skip_update_phoundry = env::var("PCL_SKIP_UPDATE_PHOUNDRY")
        .map(|val| val.to_lowercase() == "true")
        .unwrap_or(false);

    // Get the workspace root directory
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir)
        .parent() // up from bin/pcl
        .unwrap()
        .parent() // up to workspace root
        .unwrap();

    if skip_build_phoundry {
        println!("cargo:warning=Skipping building phoundry - PCL_SKIP_BUILD_PHOUNDRY is set");
        return Ok(());
    }

    if skip_update_phoundry {
        println!("cargo:warning=Skipping updating phoundry - PCL_SKIP_UPDATE_PHOUNDRY is set");
    } else {
        // Update phoundry submodule
        update_phoundry(workspace_root).expect("Failed to update phoundry submodule");
    }

    // Build phoundry/forge
    build_phoundry(workspace_root, &profile).expect("Failed to build phoundry");

    // Copy the forge binary to the main target directory instead of OUT_DIR
    let source = workspace_root
        .join("phoundry")
        .join("target")
        .join(&profile)
        .join("forge");

    let dest = workspace_root.join("target").join(&profile).join("phorge");

    println!(
        "cargo:warning=Copying {} to {}",
        source.display(),
        dest.display()
    );
    fs::copy(&source, &dest).expect("Failed to copy forge binary");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=phoundry");
    Ok(())
}

fn update_phoundry(workspace_root: &Path) -> std::io::Result<()> {
    Command::new("git")
        .current_dir(workspace_root)
        .args(["submodule", "update", "--init", "--recursive", "--remote"])
        .status()?;
    Ok(())
}

fn build_phoundry(workspace_root: &Path, mode: &str) -> std::io::Result<()> {
    let mut args = vec!["build", "--bin", "forge"];
    if mode == "release" {
        args.push("--release");
    }

    Command::new("cargo")
        .current_dir(workspace_root.join("phoundry"))
        .args(&args)
        .status()?;
    Ok(())
}
