use clap::{App, SubCommand, Arg};
use std::{fs, io};
use std::path::Path;
use crate::region::{RegionFile, RegionFileWriter, Chunk};
use std::io::Cursor;
use indicatif::ProgressBar;
use rayon::prelude::*;

mod region;

fn main() -> Result<(), io::Error> {
    let matches = App::new("anviltools")
        .subcommand(SubCommand::with_name("strip-cached-data")
            .about("Strips cached data from region files")
            .arg(Arg::with_name("input-dir")
                .required(true))
            .arg(Arg::with_name("output-dir")
                .required(true))
        )
        .get_matches();

    if let Some(matches) = matches.subcommand_matches("strip-cached-data") {
        return strip_files(matches.value_of("input-dir").unwrap(),
                    matches.value_of("output-dir").unwrap());
    }

    Ok(())
}


fn strip_files(input_path_str: &str, output_path_str: &str) -> Result<(), io::Error> {
    let input_path = Path::new(input_path_str);
    let output_path = Path::new(output_path_str);

    println!("Input directory: {:?}", &input_path);
    println!("Output directory: {:?}", &output_path);

    if !Path::exists(input_path) {
        panic!("Input directory does not exist");
    }

    if input_path == output_path {
        panic!("In-place operations are not supported")
    }

    if !Path::exists(output_path) {
        fs::create_dir_all(output_path)
            .expect("Could not create output directory");
    }

    let entries: Vec<fs::DirEntry> = fs::read_dir(input_path)?
        .into_iter()
        .collect::<Result<Vec<_>, io::Error>>()?;


    let bar = ProgressBar::new((entries.len() as u64) * 1024);

    entries.par_iter()
        .map(|entry| {
            bar.inc(1);

            let path = entry.path();

            if path.is_file() {
                strip_file(input_path, output_path, &path, &bar)
            } else {
                Ok(())
            }
        })
        .collect::<Result<(), io::Error>>()?;

    bar.finish();

    Ok(())
}

fn strip_file(input_dir: &Path, output_dir: &Path, path: &Path, bar: &ProgressBar) -> Result<(), io::Error> {
    let name = path.file_name()
        .unwrap();

    let in_region = RegionFile::open(&Path::join(input_dir, name))?;
    let mut out_region = RegionFileWriter::create(&Path::join(output_dir, name))?;

    for result in in_region.stream_chunks() {
        bar.inc(1);

        let chunk = match result? {
            Some(chunk) => chunk,
            None => continue
        };

        let stripped_chunk = strip_chunk(&chunk)?;

        out_region.add_chunk(&stripped_chunk)?;
    }

    Ok(())
}

fn strip_chunk(chunk: &Chunk) -> Result<Chunk, io::Error> {
    let mut nbt: nbt::Blob = nbt::Blob::from_reader(&mut Cursor::new(&chunk.data[..]))?;

    let level = nbt.get_mut("Level");

    let level_data = match level {
        Some(nbt::Value::Compound(map)) => map,
        _ => panic!("Could not find Level tag in chunk NBT")
    };

    level_data.remove("Heightmaps");
    level_data.remove("isLightOn");

    let mut rewritten_data: Vec<u8> = Vec::new();
    nbt.to_writer(&mut rewritten_data)?;

    let rewritten_chunk = chunk.with_data(rewritten_data);

    Ok(rewritten_chunk)
}
