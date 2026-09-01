use std::env;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use flate2::{Compression, write::GzEncoder};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("manifest path is missing")?);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("output path is missing")?);

    for input in [
        "../aon-net-admin/admin/index.html",
        "../aon-net-admin/admin/admin.css",
        "../aon-net-admin/admin/Trunk.toml",
        "../aon-net-admin/src",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    let target_dir = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or("workspace path is missing")?
        .join("target/aon-net-admin");
    let status = Command::new("trunk")
        .arg("build")
        .current_dir(manifest_dir.join("../aon-net-admin/admin"))
        .env("CARGO_TARGET_DIR", target_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CLIPPY_ARGS")
        .env_remove("NO_COLOR")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTC_WRAPPER")
        .status()?;
    if !status.success() {
        return Err(format!("Trunk admin build failed with {status}").into());
    }

    let asset_output = output.join("admin");
    fs::create_dir_all(&asset_output)?;
    let dist = manifest_dir.join("../aon-net-admin/admin/dist");
    for name in ["index.html", "admin.css", "aon-net-admin.js"] {
        let bytes = fs::read(dist.join(name))?;
        if bytes.is_empty() {
            return Err(format!("admin asset {name} is empty").into());
        }
        fs::write(asset_output.join(name), bytes)?;
    }
    let wasm = fs::read(dist.join("aon-net-admin_bg.wasm"))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&wasm)?;
    fs::write(
        asset_output.join("aon-net-admin_bg.wasm.gz"),
        encoder.finish()?,
    )?;
    Ok(())
}
