use std::fs::File;

use serde::{Serialize, Deserialize};
use xz2::bufread::XzDecoder;
use xz2::write::XzEncoder;
use std::fs;
use std::path::Path;
use std::io::{self, BufWriter, prelude::*, BufReader};
use clap::Parser;
use indicatif::{ProgressBar, MultiProgress, ProgressStyle};

use crate::region::{RegionFile, ChunkPos, RegionFileWriter, Chunk};
use lazy_static::lazy_static;

lazy_static! {
    static ref PROGRESS_STYLE: ProgressStyle = {
        ProgressStyle::default_bar()
            .template("{msg} {wide_bar} {pos}/{len}")
            .progress_chars("##-")
    };
}

#[derive(Parser)]
pub struct PackOptions {
    #[clap(long, about = "Input directory of region (.mca) files to archive")]
    input_dir: String,

    #[clap(long, about = "Output path for the archive file")]
    output_file: String,
    
    #[clap(long, default_value = "7", about = "Compression level used for creating the archive file")]
    compression_level: u32,

    #[clap(long, default_value = "0", about = "Number of threads to be used for compression (0 = single-threaded)")]
    threads: u32,

    #[clap(long, about = "Strip cached data from chunks before archiving")]
    strip: bool
}

#[derive(Serialize, Deserialize)]
struct PackHeader {
    region_count: u32
}


#[derive(Serialize, Deserialize)]
struct RegionEntry {
    x: i32,
    z: i32,
    chunk_count: u32
}

#[derive(Serialize, Deserialize)]
struct ChunkEntry {
    position: ChunkPos,
    data: Box<[u8]>
}

pub fn pack_files(options: &PackOptions) -> Result<(), io::Error> {
    let input_path = Path::new(&options.input_dir);
    let output_path = Path::new(&options.output_file);

    if !Path::exists(input_path) {
        panic!("Input directory does not exist");
    }

    let writer = BufWriter::new(File::create(output_path)?);
    let mut encoder = XzEncoder::new(writer, options.compression_level);

    let entries: Vec<fs::DirEntry> = fs::read_dir(input_path)?
        .into_iter()
        .collect::<Result<Vec<_>, io::Error>>()?;

    rmp_serde::encode::write(encoder.by_ref(), &PackHeader { region_count: entries.len() as u32 })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let progress = MultiProgress::new();
    let bar = ProgressBar::new(entries.len() as u64);
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message("Compressing regions");

    entries.iter()
        .map(|entry| {
            bar.inc(1);

            let path = entry.path();

            if path.is_file() {
                pack_file(&path, &mut encoder, &progress, options)
            } else {
                Ok(())
            }
        })
        .collect::<Result<(), io::Error>>()?;

    encoder.finish()?;

    bar.finish();

    Ok(())
}

fn pack_file<T>(path: &Path, encoder: &mut XzEncoder<T>, progress: &MultiProgress, options: &PackOptions) -> Result<(), io::Error>
    where T: io::Write
{
    let (x, z) = RegionFile::parse_name(&path.file_name().map(|f| f.to_string_lossy()).unwrap());
    let region = RegionFile::open(&path)?;

    rmp_serde::encode::write(encoder, &RegionEntry { x, z, chunk_count: region.chunk_count()? })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let bar = progress.add(ProgressBar::new(1024));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message(format!("Compressing chunks for region ({}, {})", x, z));

    for result in region.stream_chunks() {
        bar.inc(1);

        let chunk = match result? {
            Some(chunk) => {
                let data = if options.strip {
                    crate::commands::strip::strip_chunk(&chunk)?.data
                } else {
                    chunk.data
                };

                ChunkEntry { position: chunk.position, data }
            },
            None => continue
        };

        rmp_serde::encode::write(encoder, &chunk)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    }

    encoder.flush()?;
    bar.finish_and_clear();

    Ok(())
}


#[derive(Parser)]
pub struct UnpackOptions {
    #[clap(long, about = "Path of the archive file to unpack")]
    input_file: String,

    #[clap(long, about = "Directory where the unpacked region files will be saved")]
    output_dir: String
}


pub fn unpack_files(options: &UnpackOptions) -> Result<(), io::Error> {
    let input_path = Path::new(&options.input_file);
    let output_dir = Path::new(&options.output_dir);

    if !Path::exists(input_path) {
        panic!("Input directory does not exist");
    }

    let mut reader = BufReader::new(File::open(input_path)?);
    let mut decoder = XzDecoder::new(&mut reader);

    let pack_header: PackHeader = rmp_serde::decode::from_read(decoder.by_ref())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let progress = MultiProgress::new();
    let bar = progress.add(ProgressBar::new(pack_header.region_count as u64));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message("Decompressing regions");

    for _ in 0..pack_header.region_count {
        unpack_file(&output_dir, &mut decoder, &progress)?;
        bar.inc(1);
    }

    bar.finish();

    Ok(())
}

fn unpack_file<T>(dir: &Path, decoder: &mut XzDecoder<T>, progress: &MultiProgress) -> Result<(), io::Error>
    where T: io::BufRead
{
    let region_entry: RegionEntry = rmp_serde::decode::from_read(decoder.by_ref())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let region_path = dir.join(format!("f.{}.{}.mca", region_entry.x, region_entry.z));
    let mut region = RegionFileWriter::create(&region_path)?;

    let bar = progress.add(ProgressBar::new(region_entry.chunk_count as u64));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message(format!("Decompressing chunks for region ({}, {})", region_entry.x, region_entry.z));

    for _ in 0..region_entry.chunk_count {
        let chunk_entry: ChunkEntry = rmp_serde::decode::from_read(decoder.by_ref())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        
        region.add_chunk(&Chunk { position: chunk_entry.position, data: chunk_entry.data })?;
        bar.inc(1);
    }

    bar.finish_and_clear();

    Ok(())
}

