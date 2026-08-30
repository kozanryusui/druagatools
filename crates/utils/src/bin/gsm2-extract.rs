use std::{
    env, fs,
    io::{self, Write},
    path::Path,
};

use druaga_utils::{atomic_output, gsm2::Image};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .ok_or("usage: gsm2-extract INPUT.gsm OUTPUT.png")?;
    let output = args
        .next()
        .ok_or("usage: gsm2-extract INPUT.gsm OUTPUT.png")?;
    if args.next().is_some() {
        return Err("usage: gsm2-extract INPUT.gsm OUTPUT.png".into());
    }

    let data = fs::read(&input)?;
    let image = Image::parse(&data)?;
    let mut png = Vec::new();
    image.write_png(&mut png)?;
    atomic_output::write_bytes(Path::new(&output), &png)?;
    writeln!(io::stderr(), "{}x{}", image.width, image.height)?;
    Ok(())
}
