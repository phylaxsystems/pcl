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
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=OUT_DIR");

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

    let target = env::var("TARGET").ok().unwrap();

    let source = workspace_root
        .join("phoundry")
        .join("target")
        .join(&target)
        .join(&profile)
        .join("forge");

    let target_dir = get_profile_dir(&std::env::var("OUT_DIR").unwrap());
    println!("cargo:warning=Target directory: {}", target_dir);

    let dest = Path::new(&target_dir).join("phorge");

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
    let mut command = Command::new("cargo");
    command
        .current_dir(workspace_root.join("phoundry"))
        .arg("build")
        .arg("--bin")
        .arg("forge");

    if mode == "release" {
        command.arg("--release");
    }

    if let Ok(target_value) = env::var("TARGET") {
        command.arg("--target").arg(target_value);
    }
    command.status()?;
    Ok(())
}

fn get_profile_dir(out_dir: &str) -> String {
    let profile = std::env::var("PROFILE").unwrap(); // "debug" or "release"

    // Normalize path separators to forward slashes
    let normalized_path = out_dir.replace('\\', "/");

    // Split the path into components, filtering out empty strings
    let components: Vec<&str> = normalized_path
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // Find the index of the profile directory
    if let Some(profile_idx) = components.iter().position(|&c| c == profile) {
        // Reconstruct the path up to and including the profile directory
        let path_components = &components[..=profile_idx];

        // Check if original path was absolute (started with slash)
        let is_absolute = out_dir.starts_with('/')
            || (out_dir.len() >= 2 && &out_dir[..2] == "\\\\")
            || (out_dir.chars().nth(1) == Some(':')); // Windows drive letter

        if is_absolute {
            if out_dir.chars().nth(1) == Some(':') {
                // Windows path
                // Reconstruct Windows path with drive letter
                format!("{}:\\{}", components[0], path_components[1..].join("\\"))
            } else {
                // Unix absolute path
                format!("/{}", path_components.join("/"))
            }
        } else {
            // Relative path
            path_components.join("/")
        }
    } else {
        // Fallback if profile not found in path
        eprintln!("Warning: Could not find profile directory in OUT_DIR");
        out_dir.to_string()
    }
}
