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
    let build = BuildBuilder::all_build()?;
    let cargo = CargoBuilder::all_cargo()?;
    let gix = GixBuilder::all_git()?;
    let rustc = RustcBuilder::all_rustc()?;
    let si = SysinfoBuilder::all_sysinfo()?;

    Emitter::default()
        .add_instructions(&build)?
        .add_instructions(&cargo)?
        .add_instructions(&gix)?
        .add_instructions(&rustc)?
        .add_instructions(&si)?
        .emit()?;
    Ok(())
}
