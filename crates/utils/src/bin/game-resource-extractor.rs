use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

use druaga_utils::game_resource::{GameResourceError, extract_game_resources};

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = OsString>) -> Result<(), CliError> {
    let values: Vec<OsString> = args.collect();
    let info = PathBuf::from(flag_value(&values, "--info")?);
    let game = PathBuf::from(flag_value(&values, "--game")?);
    let output = PathBuf::from(flag_value(&values, "--output")?);
    let summary = extract_game_resources(&info, &game, &output)?;
    println!(
        "extracted {} files ({} bytes) to {}",
        summary.file_count,
        summary.byte_count,
        output.display()
    );
    Ok(())
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
    #[error("usage: game-resource-extractor --info INFO.DAT --game GAME.DAT --output DIRECTORY")]
    Usage,
    #[error(transparent)]
    Resource(#[from] GameResourceError),
}
