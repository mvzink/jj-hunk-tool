use sha1::{Digest, Sha1};

use crate::diff::Hunk;

/// Compute a stable, short ID for a hunk based on its file path, header, and content.
pub fn compute_id(hunk: &Hunk) -> String {
    let mut hasher = Sha1::new();
    hasher.update(hunk.file_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(hunk.header.as_bytes());
    hasher.update(b"\0");
    hasher.update(hunk.content.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..4])
}
