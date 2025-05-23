use std::{path::Path, sync::atomic::AtomicBool};

use gix_features::progress::Progress;

///
pub mod checksum {
    /// Returned by various methods to verify the checksum of a memory mapped file that might also exist on disk.
    #[derive(thiserror::Error, Debug)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error("Interrupted by user")]
        Interrupted,
        #[error("Failed to hash data")]
        Hasher(#[from] gix_hash::hasher::Error),
        #[error(transparent)]
        Verify(#[from] gix_hash::verify::Error),
    }
}

/// Returns the `index` at which the following `index + 1` value is not an increment over the value at `index`.
pub fn fan(data: &[u32]) -> Option<usize> {
    data.windows(2)
        .enumerate()
        .find_map(|(win_index, v)| (v[0] > v[1]).then_some(win_index))
}

/// Calculate the hash of the given kind by trying to read the file from disk at `data_path` or falling back on the mapped content in `data`.
/// `Ok(expected)` or [`checksum::Error::Verify`] is returned if the hash matches or mismatches.
/// If the [`checksum::Error::Interrupted`] is returned, the operation was interrupted.
pub fn checksum_on_disk_or_mmap(
    data_path: &Path,
    data: &[u8],
    expected: gix_hash::ObjectId,
    object_hash: gix_hash::Kind,
    progress: &mut dyn Progress,
    should_interrupt: &AtomicBool,
) -> Result<gix_hash::ObjectId, checksum::Error> {
    let data_len_without_trailer = data.len() - object_hash.len_in_bytes();
    let actual = match gix_hash::bytes_of_file(
        data_path,
        data_len_without_trailer as u64,
        object_hash,
        progress,
        should_interrupt,
    ) {
        Ok(id) => id,
        Err(gix_hash::io::Error::Io(err)) if err.kind() == std::io::ErrorKind::Interrupted => {
            return Err(checksum::Error::Interrupted);
        }
        Err(gix_hash::io::Error::Io(_io_err)) => {
            let start = std::time::Instant::now();
            let mut hasher = gix_hash::hasher(object_hash);
            hasher.update(&data[..data_len_without_trailer]);
            progress.inc_by(data_len_without_trailer);
            progress.show_throughput(start);
            hasher.try_finalize()?
        }
        Err(gix_hash::io::Error::Hasher(err)) => return Err(checksum::Error::Hasher(err)),
    };

    actual.verify(&expected)?;
    Ok(actual)
}
