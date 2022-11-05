use std::fs::File;
use std::sync::{Arc, Mutex};
use std::fs;
use std::path::{Path, PathBuf};
use std::io::{self, BufWriter, BufReader, Read};
use std::num::NonZeroUsize;
use clap::Parser;
use rayon::iter::{ParallelBridge, ParallelIterator};
use indicatif::{ProgressBar};

use crate::region::{RegionFile, ChunkPos, RegionFileWriter, Chunk};
use atty::Stream;

#[derive(Parser)]
pub struct PackOptions {
    #[clap(long, help = "Input directory of region (.mca) files to archive")]
    input_dir: String,

    #[clap(long, help = "Output path for the tar archive file (default is pipe to stdout)", required = false)]
    output_file: Option<String>,

    #[clap(long, help = "Strip cached data from chunks before archiving", default_value = "false")]
    strip: bool,

    #[clap(long, help = "Threads used for reading region files")]
    threads: Option<u32>,

    #[clap(long, help = "Allow binary data to be piped to a TTY")]
    ignore_tty: bool
}

pub fn pack_files(options: &PackOptions) -> Result<(), io::Error> {
    let input_path = Path::new(&options.input_dir);

    if !Path::is_dir(input_path) {
        panic!("Input directory does not exist");
    }
    
    match &options.output_file {
        Some(output_file) => {
            let output_path = Path::new(output_file);
            
            let file = File::create(output_path)?;
            let file_write = BufWriter::new(file);
    
            pack_region_directory(&mut tar::Builder::new(file_write), input_path, options)
        },
        None => {
            if atty::is(Stream::Stdout) && !options.ignore_tty {
                panic!("Refusing to pipe binary data to a terminal")
            }

            pack_region_directory(&mut tar::Builder::new(io::stdout()), input_path, options)
        }
    }
}

fn pack_region_directory<W>(archive: &mut tar::Builder<W>, input_dir: &Path, options: &PackOptions) -> Result<(), io::Error>
    where W: io::Write
{
    let files: Vec<PathBuf> = fs::read_dir(input_dir)?
        .filter_map(|entry| {
            entry
                .map(|entry| entry.path())
                .map(|path| {
                    if path.is_file() {
                        Some(path)
                    } else {
                        None
                    }
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let bar = ProgressBar::new(files.len() as u64);
    bar.set_message("Packing region files");
        
    files
        .iter()
        .try_for_each(|path| {
            bar.inc(1);
            pack_region(&path, archive, options)
        })?;

    bar.finish();

    Ok(())
}

fn pack_region<W>(path: &Path, archive: &mut tar::Builder<W>, options: &PackOptions) -> Result<(), io::Error>
    where W: io::Write
{
    let region_name = path.file_name()
        .map(|f| f.to_string_lossy())
        .unwrap();

    let region_position = RegionFile::parse_name(&region_name);
    let region_file = RegionFile::open(&path)?;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads.unwrap_or(1) as usize)
        .build()
        .unwrap();

    pool.in_place_scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);

        scope.spawn(|_| {
            region_file.stream_chunks()
                .par_bridge()
                .try_for_each(move |result| -> Result<(), io::Error> {
                    if let Some(mut chunk) = result? {
                        if options.strip {
                            chunk = crate::commands::strip::strip_chunk(&chunk)?;
                        }
    
                        tx.send(chunk)
                            .unwrap();   
                    }

                    Ok(())
                })
                .unwrap();
        });

        rx
            .iter()
            .try_for_each(|chunk: Chunk| {
                let path = format!("r.{}.{}/c.{}.{}.nbt", region_position.x, region_position.z, chunk.position.x, chunk.position.z);
                archive.append_data(&mut {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(chunk.data.len() as u64);
                    header
                }, path, &*chunk.data)
            })
            .unwrap();
    });


    Ok(())
}


#[derive(Parser)]
pub struct UnpackOptions {
    #[clap(long, help = "Path of the archive file to unpack (default is pipe from stdin)")]
    input_file: Option<String>,

    #[clap(long, help = "Directory where the unpacked region files will be saved")]
    output_dir: String,

    #[clap(long, help = "Allow binary data to be piped to a TTY")]
    ignore_tty: bool
}


pub fn unpack_files(options: &UnpackOptions) -> Result<(), io::Error> {
    let output_dir = Path::new(&options.output_dir);

    match &options.input_file {
        Some(input_path) => {
            let input_path = Path::new(input_path);

            if !Path::exists(input_path) {
                panic!("Input file does not exist");
            }
        
            let file = File::open(input_path)?;
    
            unpack_files_with_reader(&mut BufReader::new(file), output_dir)
        },
        None => {
            if atty::is(Stream::Stdin) && !options.ignore_tty {
                panic!("Refusing to pipe binary data from a terminal")
            }

            unpack_files_with_reader(&mut io::stdin(), output_dir)
        }
    }
}

struct ChunkEntry {
    data: Box<[u8]>,
    region: ChunkPos,
    chunk: ChunkPos
}

fn unpack_files_with_reader<R>(reader: &mut R, output_dir: &Path) -> Result<(), io::Error>
    where R: io::Read
{
    let mut archive = tar::Archive::new(reader);
    let output_dir = output_dir.to_owned();

    let (sender, receiver) = std::sync::mpsc::sync_channel(4);
    let region_cache: RegionFileCache = Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(8).unwrap())));

    let receive_thread = std::thread::spawn(move || -> Result<(), io::Error> {
        receiver
            .iter()
            .try_for_each(|entry| {
                unpack_file(&output_dir, region_cache.clone(), entry)
            })
    });

    for entry in archive.entries()? {
        let mut entry = entry?;

        let path = entry.path()
            .expect("Couldn't determine path of tar entry");

        let region_name = path
            .parent()
            .expect("Missing region location in tar entry path")
            .to_string_lossy();

        let chunk_name = path
            .file_name()
            .expect("Missing chunk location in tar entry path")
            .to_string_lossy();

        let region_position = RegionFile::parse_name(&region_name);
        let chunk_position = RegionFile::parse_name(&chunk_name);
        
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;

        sender.send(ChunkEntry {
            data: data.into_boxed_slice(),
            region: region_position,
            chunk: chunk_position
        }).unwrap();
    }

    receive_thread.join()
        .unwrap()?;

    Ok(())
}

use lru::LruCache;

type RegionFileCache = Arc<Mutex<LruCache<ChunkPos, Arc<Mutex<RegionFileWriter>>>>>;

fn unpack_file(output_dir: &Path, region_cache: RegionFileCache, entry: ChunkEntry) -> Result<(), io::Error>
{
    let region_writer: Arc<Mutex<RegionFileWriter>> = {
        let mut region_cache = region_cache.lock()
            .unwrap();

        match region_cache.get(&entry.region) {
            Some(r) => r.clone(),
            None => {
                let region_path = output_dir.join(
                    format!("r.{}.{}.mca", entry.region.x, entry.region.z));
    
                let writer = Arc::new(Mutex::new(RegionFileWriter::create(&region_path)?));
                region_cache.put(entry.region, writer.clone());
    
                writer
            }
        }
    };

    region_writer
        .lock()
        .unwrap()
        .add_chunk(&Chunk { position: entry.chunk, data: entry.data })?;

    Ok(())
}

