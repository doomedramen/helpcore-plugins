use serde_json::Value;

wit_bindgen::generate!({
    inline: r#"
        package helpcore:plugin;

        interface host {
            http-request: func(request-json: string) -> result<string, string>;
            data-read: func(path: string) -> result<string, string>;
            data-write: func(path: string, content: string) -> result<_, string>;
            config-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

struct QrCode;

impl Guest for QrCode {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "qr_generate" => qr_generate(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(QrCode);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

// ── Galois Field GF(256) for Reed-Solomon ─────────────────────────────────────

const EXP_TABLE: [u8; 256] = [
    1, 2, 4, 8, 16, 32, 64, 128, 29, 58, 116, 232, 205, 135, 19, 38,
    76, 152, 45, 90, 180, 117, 234, 201, 143, 3, 6, 12, 24, 48, 96, 192,
    157, 39, 78, 156, 37, 74, 148, 53, 106, 212, 181, 119, 238, 193, 159, 35,
    70, 140, 5, 10, 20, 40, 80, 160, 93, 186, 105, 210, 185, 111, 222, 161,
    95, 190, 97, 194, 153, 47, 94, 188, 101, 202, 137, 15, 30, 60, 120, 240,
    253, 231, 211, 187, 107, 214, 177, 127, 254, 225, 223, 163, 91, 182, 113, 226,
    217, 175, 67, 134, 17, 34, 68, 136, 13, 26, 52, 104, 208, 189, 103, 206,
    129, 31, 62, 124, 248, 237, 199, 147, 59, 118, 236, 197, 151, 51, 102, 204,
    133, 23, 46, 92, 184, 109, 218, 169, 79, 158, 33, 66, 132, 21, 42, 84,
    168, 77, 154, 41, 82, 164, 85, 170, 73, 146, 57, 114, 228, 213, 183, 115,
    230, 209, 191, 99, 198, 145, 63, 126, 252, 229, 215, 179, 123, 246, 241, 255,
    227, 219, 171, 75, 150, 49, 98, 196, 149, 55, 110, 220, 165, 87, 174, 65,
    130, 25, 50, 100, 200, 141, 7, 14, 28, 56, 112, 224, 221, 167, 83, 166,
    81, 162, 89, 178, 121, 242, 249, 239, 195, 155, 43, 86, 172, 69, 138, 9,
    18, 36, 72, 144, 61, 122, 244, 245, 247, 243, 251, 235, 203, 139, 11, 22,
    44, 88, 176, 125, 250, 233, 207, 131, 27, 54, 108, 216, 173, 71, 142, 1,
];

const LOG_TABLE: [u8; 256] = [
    0, 0, 1, 25, 2, 50, 26, 198, 3, 223, 51, 238, 27, 104, 199, 75,
    4, 100, 224, 14, 52, 141, 239, 129, 28, 193, 105, 248, 200, 8, 76, 113,
    5, 138, 101, 47, 225, 36, 15, 33, 53, 147, 142, 218, 240, 18, 130, 69,
    29, 181, 194, 125, 106, 39, 249, 185, 201, 154, 9, 120, 77, 228, 114, 166,
    6, 191, 139, 98, 102, 221, 48, 253, 226, 152, 37, 179, 16, 145, 34, 136,
    54, 208, 148, 206, 143, 150, 219, 189, 241, 210, 19, 92, 131, 56, 70, 64,
    30, 66, 182, 163, 195, 72, 126, 110, 107, 58, 40, 84, 250, 133, 186, 61,
    202, 94, 155, 159, 10, 21, 121, 43, 78, 212, 229, 172, 115, 243, 167, 87,
    7, 112, 192, 247, 140, 128, 99, 13, 103, 74, 222, 237, 49, 197, 254, 24,
    227, 165, 153, 119, 38, 184, 180, 124, 17, 68, 146, 217, 35, 32, 137, 46,
    55, 63, 209, 91, 149, 188, 207, 205, 144, 135, 151, 178, 220, 252, 190, 97,
    242, 86, 211, 171, 20, 42, 93, 158, 132, 60, 57, 83, 71, 109, 65, 162,
    31, 45, 67, 216, 183, 123, 164, 118, 196, 23, 73, 236, 127, 12, 111, 246,
    108, 161, 59, 82, 41, 157, 85, 170, 251, 96, 134, 177, 187, 204, 62, 90,
    203, 89, 95, 176, 156, 169, 160, 81, 11, 245, 22, 235, 122, 117, 44, 215,
    79, 174, 213, 233, 230, 231, 173, 232, 116, 214, 244, 234, 168, 80, 88, 175,
];

fn gf_mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        let sum = LOG_TABLE[a as usize] as u16 + LOG_TABLE[b as usize] as u16;
        EXP_TABLE[(sum % 255) as usize]
    }
}

fn gf_poly_mul(p: &[u8], q: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; p.len() + q.len() - 1];
    for i in 0..p.len() {
        for j in 0..q.len() {
            result[i + j] ^= gf_mul(p[i], q[j]);
        }
    }
    result
}

fn reed_solomon_generator(degree: u8) -> Vec<u8> {
    let mut gen = vec![1u8];
    for i in 0..degree {
        let factor = vec![1, EXP_TABLE[i as usize]];
        gen = gf_poly_mul(&gen, &factor);
    }
    gen
}

fn reed_solomon_encode(data: &[u8], ec_count: u8) -> Vec<u8> {
    let gen = reed_solomon_generator(ec_count);
    let mut result = vec![0u8; data.len() + ec_count as usize];
    result[..data.len()].copy_from_slice(data);

    for i in 0..data.len() {
        let factor = result[i];
        if factor != 0 {
            for j in 0..ec_count as usize {
                result[i + j + 1] ^= gf_mul(gen[j], factor);
            }
        }
    }
    result[data.len()..][..ec_count as usize].to_vec()
}

// ── QR Code constants ─────────────────────────────────────────────────────────

// Byte capacity for versions 1-6 with EC level M
const BYTE_CAPACITY: [usize; 6] = [14, 26, 42, 62, 84, 106];
// EC codewords per block at EC level M
const EC_WORDS_PER_BLOCK: [[u8; 1]; 6] = [[10], [16], [26], [18], [24], [16]];
// Number of blocks at EC level M
const NUM_BLOCKS: [usize; 6] = [1, 1, 1, 2, 2, 4];
// Data codewords per block at EC level M
const DATA_PER_BLOCK: [[usize; 1]; 6] = [[16], [28], [44], [32], [43], [27]];

fn module_size(version: usize) -> usize {
    17 + version * 4
}

fn get_version(data_len: usize) -> Result<usize, String> {
    for (i, &cap) in BYTE_CAPACITY.iter().enumerate() {
        if data_len <= cap {
            return Ok(i + 1);
        }
    }
    Err(format!(
        "Content too long ({} bytes). Maximum is {} bytes for QR code. Shorten your text.",
        data_len,
        BYTE_CAPACITY[BYTE_CAPACITY.len() - 1]
    ))
}

// ── Data encoding ─────────────────────────────────────────────────────────────

fn encode_data(text: &str, version: usize) -> Vec<u8> {
    let data: Vec<u8> = text.bytes().collect();
    let char_count = data.len();

    // Mode indicator: 0100 (byte mode) - 4 bits
    // Character count indicator: 8 bits for versions 1-9
    let cc_bits = 8;

    // Terminator and padding
    let required_bits = DATA_PER_BLOCK[version - 1][0] * NUM_BLOCKS[version - 1] * 8;
    let mut bits = Vec::with_capacity(required_bits);

    // Mode 0100
    bits.push(false); bits.push(true); bits.push(false); bits.push(false);

    // Character count (8 bits)
    for i in (0..cc_bits).rev() {
        bits.push(((char_count as u8) >> i) & 1 != 0);
    }

    // Data bytes
    for &byte in &data {
        for i in (0..8).rev() {
            bits.push(((byte) >> i) & 1 != 0);
        }
    }

    // Terminator: up to 4 zero bits
    let term = (required_bits - bits.len()).min(4);
    for _ in 0..term {
        bits.push(false);
    }

    // Pad to byte boundary
    while bits.len() % 8 != 0 {
        bits.push(false);
    }

    // Pad with alternating 0xEC and 0x11
    let pad_bytes = [0xEC, 0x11];
    let mut pad_idx = 0;
    while bits.len() < required_bits {
        let byte = pad_bytes[pad_idx % 2];
        pad_idx += 1;
        for i in (0..8).rev() {
            bits.push(((byte) >> i) & 1 != 0);
        }
    }

    // Convert bits to bytes (codewords)
    let mut codewords = Vec::new();
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (j, &bit) in chunk.iter().enumerate() {
            if bit {
                byte |= 1 << (7 - j);
            }
        }
        codewords.push(byte);
    }

    codewords
}

// ── Module placement ──────────────────────────────────────────────────────────

struct QrMatrix {
    size: usize,
    modules: Vec<Vec<bool>>,
}

impl QrMatrix {
    fn new(size: usize) -> Self {
        QrMatrix {
            size,
            modules: vec![vec![false; size]; size],
        }
    }

    fn set(&mut self, x: usize, y: usize, val: bool) {
        if x < self.size && y < self.size {
            self.modules[y][x] = val;
        }
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y][x]
    }

    fn place_finder(&mut self, x: usize, y: usize) {
        for dy in 0..7 {
            for dx in 0..7 {
                let on = (dx == 0 || dx == 6 || dy == 0 || dy == 6)
                    || (dx >= 2 && dx <= 4 && dy >= 2 && dy <= 4);
                self.set(x + dx, y + dy, on);
            }
        }
    }

    fn place_timing(&mut self) {
        for i in 8..self.size - 8 {
            self.set(i, 6, i % 2 == 0);
            self.set(6, i, i % 2 == 0);
        }
    }

    fn place_alignment(&mut self, x: usize, y: usize) {
        for dy in 0..5 {
            for dx in 0..5 {
                let on = (dx == 0 || dx == 4 || dy == 0 || dy == 4)
                    || (dx == 2 && dy == 2);
                self.set(x + dx - 2, y + dy - 2, on);
            }
        }
    }

    fn reserve_format(&mut self) {
        // Finder pattern separators
        for i in 0..8 {
            self.set(i, 7, false);
            self.set(7, i, false);
            self.set(self.size - 8, i, false);
            self.set(self.size - 1 - i, 7, false);
            self.set(i, self.size - 8, false);
            self.set(7, self.size - 1 - i, false);
        }
        // Dark module
        self.set(8, self.size - 8, true);
    }

    fn get_alignment_locations(version: usize) -> Vec<usize> {
        if version == 1 {
            return vec![];
        }
        let num = version / 7 + 2;
        let step = if version == 32 { 26 } else {
            (module_size(version) as f64 / (num as f64 + 1.0)).ceil() as usize * 2
        };

        let mut locations = Vec::new();
        let size = module_size(version);
        let last = size - 7;
        for i in (0..num).rev() {
            let pos = last - i * step;
            if pos > 6 {
                locations.push(pos);
            }
        }

        // Always have first one at 6
        if locations.last() != Some(&6) {
            // The standard defines positions differently. For simplicity:
            locations.clear();
            if version >= 2 {
                locations.push(size - 7);
                locations.push(6);
            }
        }

        locations.sort();
        locations
    }
}

fn build_matrix(version: usize, all_codewords: &[u8]) -> QrMatrix {
    let size = module_size(version);
    let mut matrix = QrMatrix::new(size);

    // Finder patterns
    matrix.place_finder(0, 0);
    matrix.place_finder(size - 7, 0);
    matrix.place_finder(0, size - 7);

    // Timing patterns
    matrix.place_timing();

    // Alignment patterns
    let alignments = QrMatrix::get_alignment_locations(version);
    for &ax in &alignments {
        for &ay in &alignments {
            // Skip if overlaps with finder patterns
            let overlaps_finder = (ax == 6 && ay == 6)
                || (ax == 6 && ay == size - 7)
                || (ax == size - 7 && ay == 6);
            if !overlaps_finder {
                matrix.place_alignment(ax, ay);
            }
        }
    }

    // Reserve format info area
    matrix.reserve_format();

    // Convert codewords to bit stream
    let mut bits = Vec::new();
    for &cw in all_codewords {
        for i in (0..8).rev() {
            bits.push(((cw >> i) & 1) != 0);
        }
    }

    // Place data modules (upward, right-to-left, zigzag)
    let mut x = size as isize - 1;
    let mut y = size as isize - 1;
    let mut going_up = true;
    let mut idx = 0;

    'outer: while x >= 0 {
        if x == 6 { x -= 1; } // Skip timing pattern column

        while y >= 0 && y < size as isize {
            for dx in 0..2 {
                let px = (x - dx) as usize;
                let py = y as usize;
                if px < size && py < size {
                    let is_reserved = (px <= 8 && py <= 8)
                        || (px <= 8 && py >= size - 8)
                        || (px >= size - 8 && py <= 8)
                        || (px == 6 || py == 6);

                    if !is_reserved {
                        if idx < bits.len() {
                            matrix.set(px, py, bits[idx]);
                            idx += 1;
                        } else {
                            break 'outer;
                        }
                    }
                }
            }
            if going_up { y -= 1; } else { y += 1; }
        }
        going_up = !going_up;
        if going_up { y = size as isize - 1; } else { y = 0; }
        x -= 2;
    }

    matrix
}

const MASK_PATTERNS: [fn(usize, usize) -> bool; 8] = [
    |i, j| (i + j) % 2 == 0,
    |i, _j| i % 2 == 0,
    |_, j| j % 3 == 0,
    |i, j| (i + j) % 3 == 0,
    |i, j| ((i / 2) + (j / 3)) % 2 == 0,
    |i, j| (i * j) % 2 + (i * j) % 3 == 0,
    |i, j| ((i * j) % 2 + (i * j) % 3) % 2 == 0,
    |i, j| ((i + j) % 2 + (i * j) % 3) % 2 == 0,
];

fn apply_mask(matrix: &QrMatrix, mask: fn(usize, usize) -> bool) -> QrMatrix {
    let size = matrix.size;
    let mut result = QrMatrix::new(size);
    for y in 0..size {
        for x in 0..size {
            let is_data = (x > 8 || y > 8)
                && (x < size - 8 || y > 8)
                && (x > 8 || y < size - 8)
                && x != 6 && y != 6;
            if is_data && matrix.get(x, y) {
                result.set(x, y, !mask(x, y));
            } else {
                result.set(x, y, matrix.get(x, y));
            }
        }
    }
    result
}

fn score_penalty(matrix: &QrMatrix) -> usize {
    let size = matrix.size;
    let mut score = 0;

    // Adjacent modules in same color
    for y in 0..size {
        let mut run = 0;
        let mut last = false;
        for x in 0..size {
            let current = matrix.get(x, y);
            if current == last {
                run += 1;
            } else {
                if run >= 5 { score += run - 2; }
                run = 1;
                last = current;
            }
        }
        if run >= 5 { score += run - 2; }
    }
    for x in 0..size {
        let mut run = 0;
        let mut last = false;
        for y in 0..size {
            let current = matrix.get(x, y);
            if current == last {
                run += 1;
            } else {
                if run >= 5 { score += run - 2; }
                run = 1;
                last = current;
            }
        }
        if run >= 5 { score += run - 2; }
    }

    // 2x2 blocks
    for y in 0..size - 1 {
        for x in 0..size - 1 {
            let val = matrix.get(x, y);
            if matrix.get(x + 1, y) == val && matrix.get(x, y + 1) == val && matrix.get(x + 1, y + 1) == val {
                score += 3;
            }
        }
    }

    // Dark/light ratio
    let total = size * size;
    let dark: usize = (0..size).map(|y| (0..size).filter(|&x| matrix.get(x, y)).count()).sum();
    let pct = dark * 100 / total;
    let deviation = if pct > 50 { pct - 50 } else { 50 - pct };
    score += deviation / 5 * 10;

    score
}

fn qr_generate(input: &Value) -> Result<String, String> {
    let text = get_str(input, "text")?;
    let format = input.get("format")
        .and_then(Value::as_str)
        .unwrap_or("png");

    let version = get_version(text.len())?;
    let data_cw = encode_data(text, version);

    // Split into blocks and generate EC codewords
    let n_blocks = NUM_BLOCKS[version - 1];
    let data_per = DATA_PER_BLOCK[version - 1][0];
    let ec_per = EC_WORDS_PER_BLOCK[version - 1][0];

    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut ec_blocks: Vec<Vec<u8>> = Vec::new();

    for b in 0..n_blocks {
        let block_data: Vec<u8> = data_cw[b * data_per..(b + 1) * data_per].to_vec();
        let ec = reed_solomon_encode(&block_data, ec_per);
        blocks.push(block_data);
        ec_blocks.push(ec);
    }

    // Interleave data
    let mut all_codewords = Vec::new();
    for i in 0..data_per {
        for block in &blocks {
            if i < block.len() {
                all_codewords.push(block[i]);
            }
        }
    }
    // Interleave EC
    for i in 0..ec_per as usize {
        for ec in &ec_blocks {
            if i < ec.len() {
                all_codewords.push(ec[i]);
            }
        }
    }

    let matrix = build_matrix(version, &all_codewords);

    // Try all masks and pick the best
    let mut _best_mask = 0;
    let mut best_score = usize::MAX;
    let mut best_matrix = None;

    for mask_idx in 0..8 {
        let masked = apply_mask(&matrix, MASK_PATTERNS[mask_idx]);
        let score = score_penalty(&masked);
        if score < best_score {
            best_score = score;
            _best_mask = mask_idx;
            best_matrix = Some(masked);
        }
    }

    let matrix = best_matrix.unwrap();
    let size = matrix.size;

    match format {
        "ascii" => {
            let mut out = String::new();
            for y in 0..size {
                for x in 0..size {
                    out.push(if matrix.get(x, y) { '█' } else { ' ' });
                    out.push(if matrix.get(x, y) { '█' } else { ' ' });
                }
                out.push('\n');
            }
            Ok(out)
        }
        "matrix" => {
            let mut out = String::with_capacity(size * (size + 1));
            for y in 0..size {
                for x in 0..size {
                    out.push(if matrix.get(x, y) { '1' } else { '0' });
                }
                out.push('\n');
            }
            Ok(out)
        }
        _ => {
            let scale = 4;
            let svg_size = size * scale + scale * 2;
            let mut svg = String::new();
            svg.push_str(&format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{0}" height="{0}" viewBox="0 0 {0} {0}"><rect width="{0}" height="{0}" fill="#fff"/>"##,
                svg_size
            ));
            for y in 0..size {
                for x in 0..size {
                    if matrix.get(x, y) {
                        svg.push_str(&format!(
                            r##"<rect x="{0}" y="{1}" width="{2}" height="{2}" fill="#000"/>"##,
                            x * scale + scale, y * scale + scale, scale
                        ));
                    }
                }
            }
            svg.push_str("</svg>");

            let b64 = base64_encode_bytes(svg.as_bytes());
            Ok(format!("data:image/svg+xml;base64,{b64}"))
        }
    }
}

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode_bytes(data: &[u8]) -> String {
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(BASE64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        result.push(if chunk.len() > 1 { BASE64_CHARS[((triple >> 6) & 0x3F) as usize] as char } else { '=' });
        result.push(if chunk.len() > 2 { BASE64_CHARS[(triple & 0x3F) as usize] as char } else { '=' });
    }
    result
}
