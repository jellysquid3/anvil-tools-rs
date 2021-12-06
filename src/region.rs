use std::convert::TryInto;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{BufReader, SeekFrom};
use std::io::prelude::*;
use std::path::{Path};

use byteorder::{BigEndian, ReadBytesExt};
use flate2::read::{GzDecoder, ZlibDecoder};
use mapr::{Mmap, MmapMut};
use flate2::write::{ZlibEncoder};
use flate2::Compression;

const ENTRY_COUNT: usize = 32 * 32;
const ENTRY_LENGTH: usize = 4;

const HEADER_SIZE: usize = ENTRY_COUNT * ENTRY_LENGTH;

const REGION_LOCATION_OFFSET: usize = 0;

const SECTOR_SIZE: usize = 4096;
const INITIAL_CAPACITY: usize = HEADER_SIZE * 2;

pub struct RegionFile {
    map: Mmap
}

impl RegionFile {
    pub fn open(path: &Path) -> Result<Self, io::Error> {
        let file = File::open(path)?;

        let map = unsafe {
            Mmap::map(&file)
        }?;

        Ok(RegionFile { map })
    }

    pub fn stream_chunks(&self) -> ChunkIterator {
        ChunkIterator::create(self)
    }

    fn get_chunk_from_index(&self, index: usize) -> Result<Option<Chunk>, io::Error> {
        let entry = self.read_entry(index)?;

        match entry {
            Some(entry) => self.get_chunk_from_entry(entry)
                .map(|chunk| Some(chunk)),
            None => Ok(None)
        }
    }

    fn get_chunk_from_entry(&self, entry: RegionEntry) -> Result<Chunk, io::Error> {
        let offset = entry.sector_index as usize * SECTOR_SIZE;
        let length = entry.sector_count as usize * SECTOR_SIZE;

        let mut reader = BufReader::with_capacity(SECTOR_SIZE,
                                                  &self.map[offset..(offset + length)]);

        let exact_length = reader.read_u32::<BigEndian>()?;

        let mut data_stream = reader.take(exact_length as u64);

        let compression_mode_int = data_stream.read_u8()?;
        let compression_mode = CompressionMode::from_int(compression_mode_int)
            .expect("Invalid compression type");

        let mut data_decompressed: Vec<u8> = Vec::new();

        match compression_mode {
            CompressionMode::Gzip => {
                GzDecoder::new(data_stream)
                    .read_to_end(&mut data_decompressed)
            },
            CompressionMode::Zlib => {
                ZlibDecoder::new(data_stream)
                    .read_to_end(&mut data_decompressed)
            }
            CompressionMode::Uncompressed => {
                data_stream
                    .read_to_end(&mut data_decompressed)
            }
        }?;

        Ok(Chunk {
            data: data_decompressed.into_boxed_slice(),
            position: entry.position.clone()
        })
    }

    fn read_entry(&self, entry_index: usize) -> Result<Option<RegionEntry>, io::Error> {
        let entry_offset = REGION_LOCATION_OFFSET + (entry_index * 4);
        let entry: [u8; 4] = self.map[entry_offset..(entry_offset + 4)]
            .try_into()
            .unwrap();

        let entry_field: u32 = u32::from_be_bytes(entry);

        if entry_field == 0 {
            return Ok(None);
        }

        let sector_index = (entry_field >> 8) & 0xFFFFFF;
        let sector_count = entry_field & 0xFF;

        let position = ChunkPos {
            x: (entry_index % 32) as i32,
            z: (entry_index / 32) as i32
        };

        Ok(Some(RegionEntry {
            position,
            sector_index,
            sector_count
        }))
    }

    pub fn parse_name(name: &str) -> ChunkPos {
        let mut values = name.split('.')
            .skip(1);
                    
        let x = values.next()
            .expect("Expected x-coordinate in file name")
            .parse::<i32>()
            .expect("Failed to parse x-coordinate");
        let z = values.next()
            .expect("Expected z-coordinate in file name")
            .parse::<i32>()
            .expect("Failed to parse z-coordinate");
        ChunkPos { x, z }
    } 
}


pub struct RegionFileWriter {
    file: File,
    header_map: MmapMut,
    used_sectors: usize,
    capacity: usize
}

impl RegionFileWriter {
    pub fn create(path: &Path) -> Result<Self, io::Error> {
        let capacity = INITIAL_CAPACITY;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        file.set_len(capacity as u64)?;

        let map = unsafe {
            MmapMut::map_mut(&file)
        }?;

        Ok(RegionFileWriter {
            file,
            header_map: map,
            used_sectors: 2,
            capacity
        })
    }

    pub fn add_chunk(&mut self, chunk: &Chunk) -> Result<(), io::Error> {
        let data = RegionFileWriter::create_chunk_data_stream(&chunk.data[..])?;

        let sector_count = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;
        let sector_index = self.used_sectors;

        self.write_data(sector_index, sector_count, &data[..])?;
        self.write_entry(RegionEntry {
            position: chunk.position,
            sector_index: sector_index as u32,
            sector_count: sector_count as u32
        })?;

        self.used_sectors += sector_count as usize;

        Ok(())
    }

    fn write_data(&mut self, sector_index: usize, sector_count: usize, data: &[u8]) -> Result<(), io::Error> {
        let sector_offset = sector_index * SECTOR_SIZE;
        let capacity = (sector_index + sector_count) * SECTOR_SIZE;

        if self.capacity < capacity {
            self.file.set_len(capacity as u64)?;

            self.capacity = capacity;
        }

        self.file.seek(SeekFrom::Start(sector_offset as u64))?;
        self.file.write_all(&data)?;

        Ok(())
    }

    fn write_entry(&mut self, entry: RegionEntry) -> Result<(), io::Error> {
        let entry_index = (entry.position.x % 32) + ((entry.position.z % 32) * 32);
        let entry_offset = (entry_index * 4) as usize;

        let entry_data = u32::to_be_bytes((entry.sector_index << 8) | entry.sector_count);

        let slice = &mut self.header_map[entry_offset..(entry_offset + 4)];
        slice.copy_from_slice(&entry_data);

        Ok(())
    }

    fn create_chunk_data_stream(chunk_data: &[u8]) -> Result<Vec<u8>, io::Error> {
        let payload = RegionFileWriter::create_compressed_chunk_payload(chunk_data)?;

        let mut header = [0u8; 4];
        header[0..4].copy_from_slice(&u32::to_be_bytes(payload.len() as u32));

        let mut data: Vec<u8> = Vec::with_capacity(header.len() + payload.len());
        data.extend_from_slice(&header);
        data.extend_from_slice(&payload);

        Ok(data)
    }

    fn create_compressed_chunk_payload(payload: &[u8]) -> Result<Vec<u8>, io::Error> {
        let mut vec = Vec::new();
        vec.push(CompressionMode::Zlib.to_int());

        let mut payload_encoder = ZlibEncoder::new(vec, Compression::best());
        payload_encoder.write_all(payload)?;
        payload_encoder.finish()
    }
}

impl Drop for RegionFileWriter {
    fn drop(&mut self) {
        self.header_map.flush()
            .unwrap();
        self.file.flush()
            .unwrap();
    }
}

enum CompressionMode {
    Gzip,
    Zlib,
    Uncompressed
}

impl CompressionMode {
    pub fn from_int(int: u8) -> Option<CompressionMode> {
        match int {
            1 => Some(CompressionMode::Gzip),
            2 => Some(CompressionMode::Zlib),
            3 => Some(CompressionMode::Uncompressed),
            _ => None
        }
    }

    pub fn to_int(&self) -> u8 {
        match self {
            CompressionMode::Gzip => 1,
            CompressionMode::Zlib => 2,
            CompressionMode::Uncompressed => 3
        }
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub data: Box<[u8]>,
    pub position: ChunkPos
}

impl Chunk {
    pub fn with_data(&self, data: Box<[u8]>) -> Self {
        Chunk { data, position: self.position }
    }
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32
}

#[derive(Copy, Clone, Debug)]
pub struct RegionEntry {
    position: ChunkPos,
    sector_index: u32,
    sector_count: u32
}

pub struct ChunkIterator<'a> {
    region: &'a RegionFile,
    index: usize
}

impl<'a> ChunkIterator<'a> {
    fn create(region: &'a RegionFile) -> ChunkIterator<'a> {
        ChunkIterator { region, index: 0 }
    }
}

impl<'a> Iterator for ChunkIterator<'a> {
    type Item = Result<Option<Chunk>, io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= ENTRY_COUNT {
            None
        } else {
            let result = self.region.get_chunk_from_index(self.index);

            self.index += 1;

            Some(result)
        }
    }
}