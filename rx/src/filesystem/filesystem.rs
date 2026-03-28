use std::path;

#[derive(Clone, Debug)]
pub struct Filesystem {
    resources_dir: path::PathBuf,
}
