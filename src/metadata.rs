use std::path::PathBuf;

pub struct MetadataState {
    pub tmp_dir: PathBuf,
}

impl MetadataState {
    pub fn new(tmp_dir: PathBuf) -> Self {
        Self { tmp_dir }
    }
}
