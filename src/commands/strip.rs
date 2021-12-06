use clap::Parser;
use std::path::Path;
use std::fs;
use std::io::{self, Cursor};

use crate::region::{RegionFile, RegionFileWriter, Chunk};

#[derive(Parser)]
pub struct Options {
    #[clap(long, about = "Input directory of region (.mca) files to strip")]
    input_dir: String,

    #[clap(long, about = "Output directory where stripped region files will be stored")]
    output_dir: String
}

pub fn strip_files(options: &Options) -> Result<(), io::Error> {
    let input_path = Path::new(&options.input_dir);
    let output_path = Path::new(&options.output_dir);

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

    fs::read_dir(input_path)?
        .try_for_each(|entry| {
            let path = entry?.path();

            if path.is_file() {
                strip_file(input_path, output_path, &path)
            } else {
                Ok(())
            }
        })
}

fn strip_file(input_dir: &Path, output_dir: &Path, path: &Path) -> Result<(), io::Error> {
    let name = path.file_name()
        .unwrap();

    let in_region = RegionFile::open(&Path::join(input_dir, name))?;
    let mut out_region = RegionFileWriter::create(&Path::join(output_dir, name))?;

    for result in in_region.stream_chunks() {
        let chunk = match result? {
            Some(chunk) => chunk,
            None => continue
        };

        let stripped_chunk = strip_chunk(&chunk)?;

        out_region.add_chunk(&stripped_chunk)?;
    }

    Ok(())
}

pub fn strip_chunk(chunk: &Chunk) -> Result<Chunk, io::Error> {
    let mut nbt: nbt::Blob = nbt::Blob::from_reader(&mut Cursor::new(&chunk.data[..]))?;

    let level = nbt.get_mut("Level");

    let level_data = match level {
        Some(nbt::Value::Compound(map)) => map,
        _ => return Ok(chunk.clone())
    };

    level_data.remove("Heightmaps");
    level_data.remove("isLightOn");

    let mut rewritten_data: Vec<u8> = Vec::new();
    nbt.to_writer(&mut rewritten_data)?;

    let rewritten_chunk = chunk.with_data(rewritten_data.into_boxed_slice());

    Ok(rewritten_chunk)
}
