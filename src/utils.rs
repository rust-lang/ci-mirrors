use sha2::{Digest, Sha256};

pub fn to_hex(sha: &Sha256) -> String {
    let sha = sha.clone().finalize();
    let bytes = sha.as_slice();
    let mut result = String::new();
    for byte in bytes {
        result.push_str(&format!("{byte:0<2x}"));
    }
    result
}
