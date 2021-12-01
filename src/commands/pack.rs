use std::fs::File;
use atty::Stream;

use serde::{Serialize, Deserialize};
use std::fs;
use std::path::Path;
use std::io::{self, BufWriter, BufReader};
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

#[derive(Parser)]
pub struct PackOptions {
    #[clap(long, about = "Input directory of region (.mca) files to archive")]
    input_dir: String,

    #[clap(long, about = "Output path for the archive file (default is pipe to stdout)", required = false)]
    output_file: Option<String>,

    #[clap(long, about = "Strip cached data from chunks before archiving")]
    strip: bool
}

pub fn pack_files(options: &PackOptions) -> Result<(), io::Error> {
    let input_path = Path::new(&options.input_dir);

    match &options.output_file {
        Some(output_file) => {
            let output_path = Path::new(output_file);

            if !Path::exists(output_path) {
                panic!("Output file does not exist");
            }
            
            let file = File::create(output_path)?;
            let mut file_write = BufWriter::new(file);
    
            pack_files_with_reader(&mut file_write, input_path, options)
        },
        None => {
            if atty::is(Stream::Stdout) {
                panic!("Refusing to pipe binary data to a terminal")
            }

            pack_files_with_reader(&mut io::stdout(), input_path, options)
        }
    }
}

fn pack_files_with_reader<W>(writer: &mut W, input_dir: &Path, options: &PackOptions) -> Result<(), io::Error>
    where W: io::Write
{
    let entries: Vec<fs::DirEntry> = fs::read_dir(input_dir)?
        .into_iter()
        .collect::<Result<Vec<_>, io::Error>>()?;

    rmp_serde::encode::write(writer.by_ref(), &PackHeader { region_count: entries.len() as u32 })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let progress = MultiProgress::new();
    let bar = ProgressBar::new(entries.len() as u64);
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message("Solidifying regions");

    entries.iter()
        .map(|entry| {
            bar.inc(1);

            let path = entry.path();

            if path.is_file() {
                pack_file(&path, writer.by_ref(), &progress, options)
            } else {
                Ok(())
            }
        })
        .collect::<Result<(), io::Error>>()?;

    writer.flush()?;

    bar.finish();

    Ok(())
}

fn pack_file<T>(path: &Path, encoder: &mut T, progress: &MultiProgress, options: &PackOptions) -> Result<(), io::Error>
    where T: io::Write
{
    let (x, z) = RegionFile::parse_name(&path.file_name().map(|f| f.to_string_lossy()).unwrap());
    let region = RegionFile::open(&path)?;

    rmp_serde::encode::write(encoder, &RegionEntry { x, z, chunk_count: region.chunk_count()? })
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let bar = progress.add(ProgressBar::new(1024));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message(format!("Solidifying chunks for region ({}, {})", x, z));

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
    #[clap(long, about = "Path of the archive file to unpack (default is pipe from stdin)")]
    input_file: Option<String>,

    #[clap(long, about = "Directory where the unpacked region files will be saved")]
    output_dir: String
}


pub fn unpack_files(options: &UnpackOptions) -> Result<(), io::Error> {
    let output_dir = Path::new(&options.output_dir);

    match &options.input_file {
        Some(input_path) => {
            let input_path = Path::new(input_path);

            if !Path::exists(input_path) {
                panic!("Input directory does not exist");
            }
        
            let file = File::open(input_path)?;
    
            unpack_files_with_reader(&mut BufReader::new(file), output_dir)
        },
        None => {
            if atty::is(Stream::Stdin) {
                panic!("Refusing to pipe binary data to a terminal")
            }

            unpack_files_with_reader(&mut io::stdin(), output_dir)
        }
    }
}

fn unpack_files_with_reader<R>(reader: &mut R, output_dir: &Path) -> Result<(), io::Error>
    where R: io::Read
{
    let pack_header: PackHeader = rmp_serde::decode::from_read(reader.by_ref())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let progress = MultiProgress::new();

    let bar = progress.add(ProgressBar::new(pack_header.region_count as u64));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message("Liquifying regions");

    for _ in 0..pack_header.region_count {
        unpack_file(&output_dir, reader.by_ref(), &progress)?;
        bar.inc(1);
    }

    bar.finish();

    Ok(())
}

fn unpack_file<R>(dir: &Path, decoder: &mut R, progress: &MultiProgress) -> Result<(), io::Error>
    where R: io::Read
{
    let region_entry: RegionEntry = rmp_serde::decode::from_read(decoder.by_ref())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let region_path = dir.join(format!("r.{}.{}.mca", region_entry.x, region_entry.z));
    let mut region = RegionFileWriter::create(&region_path)?;

    let bar = progress.add(ProgressBar::new(region_entry.chunk_count as u64));
    bar.set_style(PROGRESS_STYLE.clone());
    bar.set_message(format!("Liquifying chunks for region ({}, {})", region_entry.x, region_entry.z));

    for _ in 0..region_entry.chunk_count {
        let chunk_entry: ChunkEntry = rmp_serde::decode::from_read(decoder.by_ref())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        
        region.add_chunk(&Chunk { position: chunk_entry.position, data: chunk_entry.data })?;
        bar.inc(1);
    }

    bar.finish_and_clear();

    Ok(())
}

