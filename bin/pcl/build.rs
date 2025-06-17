use anyhow::Result;
use vergen_gix::{
    BuildBuilder,
    CargoBuilder,
    Emitter,
    GixBuilder,
    RustcBuilder,
    SysinfoBuilder,
};

pub fn main() -> Result<()> {
    // Capture DA URL from environment or use default
    let da_url = std::env::var("PCL_DA_URL")
        .unwrap_or_else(|_| "https://demo-21-assertion-da.phylax.systems".to_string());
    
    // Set the DA URL as a build-time environment variable
    println!("cargo:rustc-env=PCL_BUILD_DA_URL={}", da_url);
    
    Emitter::default()
        .add_instructions(&BuildBuilder::all_build()?)?
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .add_instructions(&GixBuilder::all_git()?)?
        .add_instructions(&RustcBuilder::all_rustc()?)?
        .add_instructions(&SysinfoBuilder::all_sysinfo()?)?
        .emit()?;
    Ok(())
}
