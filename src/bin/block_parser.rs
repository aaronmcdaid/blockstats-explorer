use bitcoin::{Block, block::Header};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use anyhow::{Result, anyhow};

pub struct BlockFileReader {
    reader: BufReader<File>,
    file_path: String,
    xor_key: [u8; 8],
}

impl BlockFileReader {
    pub fn new_with_xor_key<P: AsRef<Path>>(path: P, xor_key: [u8; 8]) -> Result<Self> {
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let file_path = path.as_ref().to_string_lossy().to_string();

        Ok(BlockFileReader {
            reader,
            file_path,
            xor_key,
        })
    }

    fn deobfuscate_data(&self, data: &mut [u8], offset: u64) {
        for (i, byte) in data.iter_mut().enumerate() {
            *byte ^= self.xor_key[(offset as usize + i) % 8];
        }
    }

    pub fn read_next_block(&mut self) -> Result<Option<(Block, u64)>> {
        let current_offset = self.reader.stream_position()?;

        // Read magic bytes + size (8 bytes total)
        let mut header_bytes = [0u8; 8];
        match self.reader.read_exact(&mut header_bytes) {
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        // Deobfuscate the header
        self.deobfuscate_data(&mut header_bytes, current_offset);

        // Extract magic bytes and size
        let magic_bytes = &header_bytes[0..4];
        let size_bytes = &header_bytes[4..8];

        // Check magic bytes for mainnet (0xf9beb4d9)
        if magic_bytes != [0xf9, 0xbe, 0xb4, 0xd9] {
            return Err(anyhow!("Invalid magic bytes at offset {}: {:02x?}", current_offset, magic_bytes));
        }

        let block_size = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) as usize;


        // Read block data
        let mut block_data = vec![0u8; block_size];
        self.reader.read_exact(&mut block_data)?;

        // Deobfuscate the block data
        self.deobfuscate_data(&mut block_data, current_offset + 8);

        // Parse block using bitcoin crate
        let block: Block = bitcoin::consensus::deserialize(&block_data)?;

        Ok(Some((block, current_offset)))
    }

    pub fn read_next_header(&mut self) -> Result<Option<(Header, u64, u32)>> {
        // Block file format: [4 bytes magic][4 bytes size][80 bytes header][variable transactions]
        let current_offset = self.reader.stream_position()?;

        // Read magic bytes + size (8 bytes total)
        let mut magic_and_size = [0u8; 8];
        match self.reader.read_exact(&mut magic_and_size) {
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        // Check if we hit padding (all zeros) before deobfuscation
        if magic_and_size == [0; 8] {
            return Ok(None); // End of valid blocks, continue normally
        }

        // Deobfuscate the magic and size
        self.deobfuscate_data(&mut magic_and_size, current_offset);

        // Extract magic bytes and size
        let magic_bytes = &magic_and_size[0..4];
        let size_bytes = &magic_and_size[4..8];

        // Check magic bytes for mainnet (0xf9beb4d9)
        if magic_bytes != [0xf9, 0xbe, 0xb4, 0xd9] {
            return Err(anyhow!("Invalid magic bytes at offset {}: {:02x?}", current_offset, magic_bytes));
        }

        let block_size = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) as usize;


        // Read the block header (80 bytes)
        let mut header_data = [0u8; 80];
        self.reader.read_exact(&mut header_data)?;

        // Deobfuscate the header data
        self.deobfuscate_data(&mut header_data, current_offset + 8);

        // Parse header using bitcoin crate
        let header: Header = bitcoin::consensus::deserialize(&header_data)?;

        // Skip the remaining transaction data
        let remaining_bytes = block_size - 80;
        self.reader.seek(SeekFrom::Current(remaining_bytes as i64))?;


        Ok(Some((header, current_offset, block_size as u32)))
    }

    pub fn seek_to_offset(&mut self, offset: u64) -> Result<()> {
        self.reader.seek(SeekFrom::Start(offset))?;
        Ok(())
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }
}