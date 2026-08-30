use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

use druaga_utils::catalog::{CatalogError, load_catalog};
use druaga_utils::inspection::{
    InspectionError, execute_inspection_branch, finalize_inspection_branch, load_inspection,
    require_complete,
};
use druaga_utils::ps2mc::{InspectionLimits, Ps2McError, inspect_dongle};
use druaga_utils::render::{render_markdown, write_markdown};

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(event) => {
            println!("{event}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = OsString>) -> Result<&'static str, CliError> {
    let values: Vec<OsString> = args.collect();
    let command = values
        .first()
        .ok_or(CliError::Usage)?
        .to_str()
        .ok_or(CliError::NonUtf8Command)?;

    match command {
        "validate" => {
            let catalog_path = flag_value(&values, "--catalog")?;
            load_catalog(&PathBuf::from(catalog_path))?;
            Ok("catalog_validated")
        }
        "render" => {
            let catalog_path = flag_value(&values, "--catalog")?;
            let catalog = load_catalog(&PathBuf::from(catalog_path))?;
            let output = flag_value(&values, "--output")?;
            write_markdown(&PathBuf::from(output), &render_markdown(&catalog))?;
            Ok("markdown_rendered")
        }
        "validate-inspection-branch" => {
            let manifest = PathBuf::from(flag_value(&values, "--manifest")?);
            if values.iter().any(|value| value == OsStr::new("--finalize")) {
                finalize_inspection_branch(&manifest)?;
                Ok("inspection_branch_finalized")
            } else if values
                .iter()
                .any(|value| value == OsStr::new("--require-complete"))
            {
                require_complete(&manifest)?;
                Ok("inspection_branch_complete")
            } else {
                load_inspection(&manifest)?;
                Ok("inspection_branch_validated")
            }
        }
        "execute-inspection-branch" => {
            let manifest = PathBuf::from(flag_value(&values, "--manifest")?);
            execute_inspection_branch(&manifest)?;
            Ok("inspection_branch_executed")
        }
        "inspect-dongle" => {
            let input = PathBuf::from(flag_value(&values, "--input")?);
            let output = PathBuf::from(flag_value(&values, "--output")?);
            let max_depth = parse_limit(&values, "--max-depth")?;
            let max_entries = parse_limit(&values, "--max-entries")?;
            inspect_dongle(
                &input,
                &output,
                InspectionLimits::new(max_depth, max_entries, max_entries.max(1)),
            )?;
            Ok("dongle_structure_listed")
        }
        _ => Err(CliError::Usage),
    }
}

fn parse_limit(values: &[OsString], flag: &'static str) -> Result<usize, CliError> {
    flag_value(values, flag)?
        .to_str()
        .ok_or(CliError::NonUtf8Scalar { flag })?
        .parse()
        .map_err(|source| CliError::InvalidLimit { flag, source })
}

fn flag_value<'a>(values: &'a [OsString], flag: &str) -> Result<&'a OsStr, CliError> {
    values
        .windows(2)
        .find(|pair| pair[0] == OsStr::new(flag))
        .map(|pair| pair[1].as_os_str())
        .ok_or(CliError::Usage)
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(
        "usage: artifact-catalog <validate|render|validate-inspection-branch|execute-inspection-branch|inspect-dongle> [options]"
    )]
    Usage,
    #[error("command is not valid UTF-8")]
    NonUtf8Command,
    #[error("value for {flag} is not valid UTF-8")]
    NonUtf8Scalar { flag: &'static str },
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    #[error(transparent)]
    Ps2Mc(#[from] Ps2McError),
    #[error("invalid numeric value for {flag}: {source}")]
    InvalidLimit {
        flag: &'static str,
        #[source]
        source: std::num::ParseIntError,
    },
}
