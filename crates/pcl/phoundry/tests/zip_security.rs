use std::{
    fs,
    io::Write,
};
use zip::{
    ZipWriter,
    write::SimpleFileOptions,
};

#[tokio::test]
async fn dependency_zip_preserves_flat_layout_and_strips_single_root()
-> Result<(), Box<dyn std::error::Error>> {
    for name in ["contract.sol", "dependency/contract.sol"] {
        let temp = tempfile::tempdir()?;
        let archive = temp.path().join("dependency.zip");
        let output = temp.path().join("output");
        let mut writer = ZipWriter::new(fs::File::create(&archive)?);
        writer.start_file(name, SimpleFileOptions::default())?;
        writer.write_all(b"contract Safe {}")?;
        writer.finish()?;

        soldeer_core::download::unzip_file(&archive, &output).await?;

        assert_eq!(fs::read(output.join("contract.sol"))?, b"contract Safe {}");
        assert!(
            !archive.exists(),
            "successful extraction removes the archive"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn dependency_zip_cannot_write_through_an_absolute_symlink()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let archive = temp.path().join("dependency.zip");
    let output = temp.path().join("output");
    let outside = temp.path().join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("victim.sol"), b"unchanged")?;
    let mut writer = ZipWriter::new(fs::File::create(&archive)?);
    let options = SimpleFileOptions::default();
    writer.start_file("dependency/contract.sol", options)?;
    writer.write_all(b"contract Safe {}")?;
    writer.add_symlink("dependency/escape", outside.to_string_lossy(), options)?;
    writer.start_file("dependency/escape/victim.sol", options)?;
    writer.write_all(b"overwritten")?;
    writer.finish()?;

    let result = soldeer_core::download::unzip_file(&archive, &output).await;

    assert!(result.is_err(), "unsafe symlink extraction must fail");
    assert_eq!(fs::read(outside.join("victim.sol"))?, b"unchanged");
    assert!(
        archive.exists(),
        "failed extraction retains the input archive"
    );
    Ok(())
}
