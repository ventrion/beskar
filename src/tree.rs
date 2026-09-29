use crate::format::Result;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Missing,
    Directory(String),
    File(String),
    Symlink(PathBuf),
    Unsupported,
}

pub fn inspect_target(path: &Path) -> Result<Target> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Target::Missing),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let ty = metadata.file_type();
    if ty.is_dir() {
        Ok(Target::Directory(fingerprint(path)?))
    } else if ty.is_file() {
        let mut hash = Sha256::new();
        hash.update(b"file");
        hash_file(path, &mut hash)?;
        Ok(Target::File(hex(hash.finish())))
    } else if ty.is_symlink() {
        Ok(Target::Symlink(
            fs::read_link(path).map_err(|e| format!("{}: {e}", path.display()))?,
        ))
    } else {
        Ok(Target::Unsupported)
    }
}

pub fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn kind(path: &Path) -> Result<fs::FileType> {
    let ty = fs::symlink_metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .file_type();
    if ty.is_symlink() {
        return Err(format!(
            "symlinks are not supported in skills: {}",
            path.display()
        ));
    }
    if !ty.is_file() && !ty.is_dir() {
        return Err(format!("unsupported file type: {}", path.display()));
    }
    Ok(ty)
}

pub fn fingerprint(dir: &Path) -> Result<String> {
    if !kind(dir)?.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let mut hash = Sha256::new();
    walk_hash(dir, dir, &mut hash)?;
    Ok(hex(hash.finish()))
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn walk_hash(root: &Path, dir: &Path, hash: &mut Sha256) -> Result<()> {
    for path in entries(dir)? {
        let ty = kind(&path)?;
        let relative = path.strip_prefix(root).unwrap();
        let name = relative
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 path: {}", path.display()))?;
        hash.update(&[if ty.is_dir() { b'D' } else { b'F' }]);
        hash.update(&(name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        if ty.is_dir() {
            walk_hash(root, &path, hash)?;
        } else {
            hash_file(&path, hash)?;
        }
    }
    Ok(())
}

fn hash_file(path: &Path, hash: &mut Sha256) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .permissions()
            .mode();
        hash.update(&[(mode & 0o111) as u8]);
    }
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    hash.update(&length.to_be_bytes());
    let mut buffer = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(())
}

pub fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    if !kind(source)?.is_dir() {
        return Err(format!("not a directory: {}", source.display()));
    }
    fs::create_dir(target).map_err(|e| format!("{}: {e}", target.display()))?;
    copy_contents(source, target)
}

fn copy_contents(source: &Path, target: &Path) -> Result<()> {
    for path in entries(source)? {
        let ty = kind(&path)?;
        let destination = target.join(path.file_name().unwrap());
        if ty.is_dir() {
            fs::create_dir(&destination).map_err(|e| format!("{}: {e}", destination.display()))?;
            copy_contents(&path, &destination)?;
        } else {
            fs::copy(&path, &destination)
                .map_err(|e| format!("{} -> {}: {e}", path.display(), destination.display()))?;
        }
    }
    Ok(())
}

pub fn temporary_dir(parent: &Path, label: &str) -> Result<PathBuf> {
    for n in 0..100 {
        let path = parent.join(format!(".beskar-{}-{label}-{n}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Err("cannot reserve temporary directory".into())
}

/// Small, dependency-free SHA-256 implementation for deterministic fingerprints.
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    used: usize,
    length: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            used: 0,
            length: 0,
        }
    }
    fn update(&mut self, bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        for &byte in bytes {
            self.buffer[self.used] = byte;
            self.used += 1;
            if self.used == 64 {
                self.block();
                self.used = 0;
            }
        }
    }
    fn finish(mut self) -> [u8; 32] {
        let bit_len = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != 56 {
            self.update(&[0]);
        }
        self.update(&bit_len.to_be_bytes());
        let mut out = [0; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
    fn block(&mut self) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (i, chunk) in self.buffer.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_vectors() {
        let empty = Sha256::new().finish();
        assert_eq!(
            empty.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.finish()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut h = Sha256::new();
        h.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            h.finish()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
