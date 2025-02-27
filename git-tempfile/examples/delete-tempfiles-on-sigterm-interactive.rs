use std::path::PathBuf;

use git_tempfile::{AutoRemove, ContainingDirectory};

fn main() -> std::io::Result<()> {
    git_tempfile::setup(Default::default());
    let filepath = PathBuf::new().join("writable-tempfile.ext");
    let markerpath = PathBuf::new().join("marker.ext");
    let _tempfile = git_tempfile::writable_at(&filepath, ContainingDirectory::Exists, AutoRemove::Tempfile)?;
    let _markerfile = git_tempfile::mark_at(&markerpath, ContainingDirectory::Exists, AutoRemove::Tempfile)?;
    eprintln!(
        "Observe the tempfiles at {} and {} and hit Ctrl+C to see it vanish. I will go to sleep now…",
        filepath.display(),
        markerpath.display()
    );
    std::thread::sleep(std::time::Duration::from_secs(8 * 60 * 60));
    Ok(())
}
