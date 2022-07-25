

pub fn compress(uncompressed: Vec<u8>) -> Vec<u8>{
    let mut encoder = snap::Encoder::new();

    let mut compressed = encoder.compress_vec(&uncompressed).unwrap();

    println!("Compressed {} to {}", uncompressed.len(), compressed.len());
    return compressed;
}
pub fn decompress(compressed: Vec<u8>) -> Vec<u8>{
    let mut decoder = snap::Decoder::new();
    let uncompressed = decoder.decompress_vec(&compressed).unwrap();
    println!("Decompressed {} from {}", uncompressed.len(), compressed.len());
    return uncompressed;
}