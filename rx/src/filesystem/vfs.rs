use std::{
    collections::VecDeque,
    fmt::{self, Debug},
    fs,
    io::{self, Read, Seek, Write},
    path::{self, Path, PathBuf},
    sync::Mutex,
};

use crate::error::{GameError, GameResult};

fn convenient_path_to_str(path: &path::Path) -> GameResult<&str> {
    path.to_str().ok_or_else(|| {
        let errmessage = format!("Invalid path format for resource: {path:?}");
        GameError::FilesystemError(errmessage)
    })
}

pub trait VFile: Read + Write + Seek + Debug {}

impl<T> VFile for T where T: Read + Write + Seek + Debug {}

#[must_use]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    create: bool,
    append: bool,
    truncate: bool,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        Self::default()
    }

    /// Open for reading
    pub fn read(mut self, read: bool) -> OpenOptions {
        self.read = read;
        self
    }

    /// Open for writing
    pub fn write(mut self, write: bool) -> OpenOptions {
        self.write = write;
        self
    }

    /// Create the file if it does not exist yet
    pub fn create(mut self, create: bool) -> OpenOptions {
        self.create = create;
        self
    }

    /// Append at the end of the file
    pub fn append(mut self, append: bool) -> OpenOptions {
        self.append = append;
        self
    }

    /// Truncate the file to 0 bytes after opening
    pub fn truncate(mut self, truncate: bool) -> OpenOptions {
        self.truncate = truncate;
        self
    }

    fn to_fs_openoptions(self) -> fs::OpenOptions {
        let mut opt = fs::OpenOptions::new();
        let _ = opt
            .read(self.read)
            .write(self.write)
            .create(self.create)
            .append(self.append)
            .truncate(self.truncate)
            .create(self.create);
        opt
    }
}

#[allow(clippy::upper_case_acronyms)]
pub trait VFS: Debug + Send + Sync {
    /// Open the file at this path with the given options
    fn open_options(&self, path: &Path, open_options: OpenOptions) -> GameResult<Box<dyn VFile>>;
    /// Open the file at this path for reading
    fn open(&self, path: &Path) -> GameResult<Box<dyn VFile>> {
        self.open_options(path, OpenOptions::new().read(true))
    }
    /// Open the file at this path for writing, truncating it if it exists already
    fn create(&self, path: &Path) -> GameResult<Box<dyn VFile>> {
        self.open_options(
            path,
            OpenOptions::new().write(true).create(true).truncate(true),
        )
    }
    /// Open the file at this path for appending, creating it if necessary
    #[allow(dead_code)]
    fn append(&self, path: &Path) -> GameResult<Box<dyn VFile>> {
        self.open_options(
            path,
            OpenOptions::new().write(true).create(true).append(true),
        )
    }
    /// Create a directory at the location by this path
    fn mkdir(&self, path: &Path) -> GameResult;

    /// Remove a file or an empty directory.
    fn rm(&self, path: &Path) -> GameResult;

    /// Remove a file or directory and all its contents
    fn rmrf(&self, path: &Path) -> GameResult;

    /// Check if the file exists
    fn exists(&self, path: &Path) -> bool;

    /// Get the file's metadata
    fn metadata(&self, path: &Path) -> GameResult<Box<dyn VMetadata>>;

    /// Retrieve all file and directory entries in the given directory.
    fn read_dir(&self, path: &Path, dst: &mut Vec<PathBuf>) -> GameResult<()>;

    /// Retrieve the actual location of the VFS root, if available.
    fn to_path_buf(&self) -> Option<PathBuf>;
}

pub trait VMetadata {
    fn is_dir(&self) -> bool;
    fn is_file(&self) -> bool;
    /// Returns the length. If it is a directory,
    /// the result of this is undefined/platform dependent.
    #[allow(dead_code)]
    fn len(&self) -> u64;
}

/// A VFS that points to a directory and uses it as the root of its
/// file hierarchy.
#[derive(Clone)]
#[allow(clippy::upper_case_acronyms)]
pub struct PhysicalFS {
    root: PathBuf,
    readonly: bool,
}

#[derive(Debug, Clone)]
pub struct PhysicalMetadata(fs::Metadata);

impl VMetadata for PhysicalMetadata {
    fn is_dir(&self) -> bool {
        self.0.is_dir()
    }
    fn is_file(&self) -> bool {
        self.0.is_file()
    }
    fn len(&self) -> u64 {
        self.0.len()
    }
}

fn sanitize_path(path: &path::Path) -> Option<PathBuf> {
    let mut c = path.components();
    match c.next() {
        Some(path::Component::RootDir) => (),
        _ => return None,
    }

    fn is_normal_component(comp: path::Component<'_>) -> Option<&str> {
        match comp {
            path::Component::Normal(s) => s.to_str(),
            _ => None,
        }
    }

    // This could be done more cleverly but meh
    let mut accm = PathBuf::new();
    for component in c {
        if let Some(s) = is_normal_component(component) {
            accm.push(s);
        } else {
            return None;
        }
    }
    Some(accm)
}

fn sanitize_path_for_zip(path: &path::Path) -> Option<String> {
    let mut c = path.components();
    match c.next() {
        Some(path::Component::RootDir) => (),
        _ => return None,
    }

    fn is_normal_component(comp: path::Component<'_>) -> Option<&str> {
        match comp {
            path::Component::Normal(s) => s.to_str(),
            _ => None,
        }
    }

    // This could be done more cleverly but meh
    let mut accm = String::new();
    for component in c {
        if let Some(s) = is_normal_component(component) {
            accm.push_str(s);
            accm.push('/');
        } else {
            return None;
        }
    }
    let accm = accm.trim_end_matches('/').to_string();
    Some(accm)
}

impl PhysicalFS {
    pub fn new(root: &Path, readonly: bool) -> Self {
        PhysicalFS {
            root: root.into(),
            readonly,
        }
    }

    fn to_absolute(&self, p: &Path) -> GameResult<PathBuf> {
        if let Some(safe_path) = sanitize_path(p) {
            let mut root_path = self.root.clone();
            root_path.push(safe_path);
            Ok(root_path)
        } else {
            let msg = format!(
                "Path {p:?} is not valid: must be an absolute path with no \
                 references to parent directories"
            );
            Err(GameError::FilesystemError(msg))
        }
    }

    fn create_root(&self) -> GameResult {
        if !self.root.exists() {
            fs::create_dir_all(&self.root).map_err(GameError::from)
        } else {
            Ok(())
        }
    }
}

impl Debug for PhysicalFS {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "<PhysicalFS root: {}>", self.root.display())
    }
}

impl VFS for PhysicalFS {
    /// Open the file at this path with the given options
    fn open_options(&self, path: &Path, open_options: OpenOptions) -> GameResult<Box<dyn VFile>> {
        if self.readonly
            && (open_options.write
                || open_options.create
                || open_options.append
                || open_options.truncate)
        {
            let msg = format!("Cannot alter file {path:?} in root {self:?}, filesystem read-only");
            return Err(GameError::FilesystemError(msg));
        }

        if open_options.create {
            self.create_root()?;
        }

        let p = self.to_absolute(path)?;
        open_options
            .to_fs_openoptions()
            .open(p)
            .map(|x| Box::new(x) as Box<dyn VFile>)
            .map_err(GameError::from)
    }

    /// Create a directory at the location by this path
    fn mkdir(&self, path: &Path) -> GameResult {
        if self.readonly {
            return Err(GameError::FilesystemError(
                "Tried to make directory {} but FS is \
                 read-only"
                    .to_string(),
            ));
        }

        self.create_root()?;

        let p = self.to_absolute(path)?;
        fs::DirBuilder::new()
            .recursive(true)
            .create(p)
            .map_err(GameError::from)
    }

    /// Remove a file
    fn rm(&self, path: &Path) -> GameResult {
        if self.readonly {
            return Err(GameError::FilesystemError(
                "Tried to remove file {} but FS is read-only".to_string(),
            ));
        }

        let p = self.to_absolute(path)?;
        if p.is_dir() {
            fs::remove_dir(p).map_err(GameError::from)
        } else {
            fs::remove_file(p).map_err(GameError::from)
        }
    }

    /// Remove a file or directory and all its contents
    fn rmrf(&self, path: &Path) -> GameResult {
        if self.readonly {
            return Err(GameError::FilesystemError(
                "Tried to remove file/dir {} but FS is \
                 read-only"
                    .to_string(),
            ));
        }

        let p = self.to_absolute(path)?;
        if p.is_dir() {
            fs::remove_dir_all(p).map_err(GameError::from)
        } else {
            fs::remove_file(p).map_err(GameError::from)
        }
    }

    /// Check if the file exists
    fn exists(&self, path: &Path) -> bool {
        match self.to_absolute(path) {
            Ok(p) => p.exists(),
            _ => false,
        }
    }

    /// Get the file's metadata
    fn metadata(&self, path: &Path) -> GameResult<Box<dyn VMetadata>> {
        let p = self.to_absolute(path)?;
        p.metadata()
            .map(|m| Box::new(PhysicalMetadata(m)) as Box<dyn VMetadata>)
            .map_err(GameError::from)
    }

    /// Retrieve the path entries in this path
    fn read_dir(&self, path: &Path, dst: &mut Vec<PathBuf>) -> GameResult<()> {
        let p = self.to_absolute(path)?;
        let direntry_to_path = |entry: fs::DirEntry| -> PathBuf {
            let mut pathbuf = PathBuf::from(path);
            pathbuf.push(entry.file_name());
            pathbuf
        };

        for entry in fs::read_dir(p)? {
            dst.push(direntry_to_path(entry?));
        }

        Ok(())
    }

    /// Retrieve the actual location of the VFS root, if available.
    fn to_path_buf(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }
}

/// Joins several VFS's together in order.
#[derive(Debug)]
#[allow(clippy::upper_case_acronyms)]
pub struct OverlayFS {
    roots: VecDeque<Box<dyn VFS>>,
}

impl OverlayFS {
    pub fn new() -> Self {
        Self {
            roots: VecDeque::new(),
        }
    }

    /// Adds a new VFS to the front of the list.
    #[allow(dead_code)]
    pub fn push_front(&mut self, fs: Box<dyn VFS>) {
        self.roots.push_front(fs);
    }

    /// Adds a new VFS to the end of the list.
    pub fn push_back(&mut self, fs: Box<dyn VFS>) {
        self.roots.push_back(fs);
    }

    pub fn roots(&self) -> &VecDeque<Box<dyn VFS>> {
        &self.roots
    }
}

impl VFS for OverlayFS {
    /// Open the file at this path with the given options
    fn open_options(&self, path: &Path, open_options: OpenOptions) -> GameResult<Box<dyn VFile>> {
        let mut tried: Vec<(PathBuf, GameError)> = vec![];

        for vfs in &self.roots {
            match vfs.open_options(path, open_options) {
                Err(e) => {
                    if let Some(vfs_path) = vfs.to_path_buf() {
                        tried.push((vfs_path, e));
                    } else {
                        tried.push((PathBuf::from("<invalid path>"), e));
                    }
                }
                f => return f,
            }
        }
        let errmessage = String::from(convenient_path_to_str(path)?);
        Err(GameError::ResourceNotFound(errmessage, tried))
    }

    /// Create a directory at the location by this path
    fn mkdir(&self, path: &Path) -> GameResult {
        for vfs in &self.roots {
            match vfs.mkdir(path) {
                Err(_) => (),
                f => return f,
            }
        }
        Err(GameError::FilesystemError(format!(
            "Could not find anywhere writeable to make dir {path:?}"
        )))
    }

    /// Remove a file
    fn rm(&self, path: &Path) -> GameResult {
        for vfs in &self.roots {
            match vfs.rm(path) {
                Err(_) => (),
                f => return f,
            }
        }
        Err(GameError::FilesystemError(format!(
            "Could not remove file {path:?}"
        )))
    }

    /// Remove a file or directory and all its contents
    fn rmrf(&self, path: &Path) -> GameResult {
        for vfs in &self.roots {
            match vfs.rmrf(path) {
                Err(_) => (),
                f => return f,
            }
        }
        Err(GameError::FilesystemError(format!(
            "Could not remove file/dir {path:?}"
        )))
    }

    /// Check if the file exists
    fn exists(&self, path: &Path) -> bool {
        for vfs in &self.roots {
            if vfs.exists(path) {
                return true;
            }
        }

        false
    }

    /// Get the file's metadata
    fn metadata(&self, path: &Path) -> GameResult<Box<dyn VMetadata>> {
        for vfs in &self.roots {
            match vfs.metadata(path) {
                Err(_) => (),
                f => return f,
            }
        }
        Err(GameError::FilesystemError(format!(
            "Could not get metadata for file/dir {path:?}"
        )))
    }

    /// Retrieve the path entries in this path
    fn read_dir(&self, path: &Path, dst: &mut Vec<PathBuf>) -> GameResult<()> {
        for fs in &self.roots {
            let _ = fs.read_dir(path, dst);
        }
        Ok(())
    }

    /// Retrieve the actual location of the VFS root, if available.
    fn to_path_buf(&self) -> Option<PathBuf> {
        None
    }
}

pub trait ReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send> ReadSeek for T {}

impl Debug for dyn ReadSeek {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "<reader>")
    }
}

/// A filesystem backed by a zip file.
#[derive(Debug)]
#[allow(clippy::upper_case_acronyms)]
pub struct ZipFS {
    source: Option<PathBuf>,
    archive: Mutex<zip::ZipArchive<Box<dyn ReadSeek>>>,
    // We keep an index of what files are in the zip file
    index: Vec<String>,
}

impl ZipFS {
    pub fn new(filename: &Path) -> GameResult<Self> {
        let f = fs::File::open(filename)?;
        Self::from_reader(Box::new(f), Some(filename.into()))
    }

    /// Creates a `ZipFS` from any `Read+Seek` object, most useful with an
    /// in-memory `std::io::Cursor`.
    pub fn from_read<R>(reader: R) -> GameResult<Self>
    where
        R: Read + Seek + Send + 'static,
    {
        Self::from_reader(Box::new(reader), None)
    }

    fn from_reader(reader: Box<dyn ReadSeek>, source: Option<PathBuf>) -> GameResult<Self> {
        let mut archive = zip::ZipArchive::new(reader)?;

        let index = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();

        Ok(Self {
            source,
            archive: Mutex::new(archive),
            index,
        })
    }
}

#[derive(Clone)]
pub struct ZipFileWrapper {
    buffer: io::Cursor<Vec<u8>>,
}

impl ZipFileWrapper {
    fn new(z: &mut zip::read::ZipFile<Box<dyn ReadSeek>>) -> GameResult<Self> {
        let mut b = Vec::new();
        let _ = z.read_to_end(&mut b)?;
        Ok(Self {
            buffer: io::Cursor::new(b),
        })
    }
}

impl io::Read for ZipFileWrapper {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.buffer.read(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.buffer.read_exact(buf)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.buffer.read_to_end(buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.buffer.read_to_string(buf)
    }
}

impl io::Write for ZipFileWrapper {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "cannot write to a zip file!",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Seek for ZipFileWrapper {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.buffer.seek(pos)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        self.buffer.stream_position()
    }
}

impl Debug for ZipFileWrapper {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "<Zipfile>")
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct ZipMetadata {
    len: u64,
    is_dir: bool,
    is_file: bool,
}

impl ZipMetadata {
    fn new(name: &str, archive: &mut zip::ZipArchive<Box<dyn ReadSeek>>) -> Option<Self> {
        match archive_get_by_name(archive, name) {
            Err(_) => None,
            Ok(zipfile) => {
                let len = zipfile.size();
                Some(ZipMetadata {
                    len,
                    is_file: true,
                    is_dir: false, // mu
                })
            }
        }
    }
}

impl VMetadata for ZipMetadata {
    fn is_dir(&self) -> bool {
        self.is_dir
    }
    fn is_file(&self) -> bool {
        self.is_file
    }
    fn len(&self) -> u64 {
        self.len
    }
}

fn archive_get_by_name<'a>(
    archive: &'a mut zip::ZipArchive<Box<dyn ReadSeek>>,
    name: &str,
) -> zip::result::ZipResult<zip::read::ZipFile<'a, Box<dyn ReadSeek>>> {
    let filename =
        sanitize_path_for_zip(Path::new(name)).ok_or(zip::result::ZipError::FileNotFound)?;
    archive.by_name(&filename)
}

impl VFS for ZipFS {
    fn open_options(&self, path: &Path, open_options: OpenOptions) -> GameResult<Box<dyn VFile>> {
        // Zip is readonly
        let path = convenient_path_to_str(path)?;
        if open_options.write || open_options.create || open_options.append || open_options.truncate
        {
            let msg =
                format!("Cannot alter file {path:?} in zipfile {self:?}, filesystem read-only");
            return Err(GameError::FilesystemError(msg));
        }
        let mut archive = self.archive.lock().unwrap();
        let mut f = archive_get_by_name(&mut archive, path)?;
        let zipfile = ZipFileWrapper::new(&mut f)?;
        Ok(Box::new(zipfile) as Box<dyn VFile>)
    }

    fn mkdir(&self, path: &Path) -> GameResult {
        let msg = format!("Cannot mkdir {path:?} in zipfile {self:?}, filesystem read-only");
        Err(GameError::FilesystemError(msg))
    }

    fn rm(&self, path: &Path) -> GameResult {
        let msg = format!("Cannot rm {path:?} in zipfile {self:?}, filesystem read-only");
        Err(GameError::FilesystemError(msg))
    }

    fn rmrf(&self, path: &Path) -> GameResult {
        let msg = format!("Cannot rmrf {path:?} in zipfile {self:?}, filesystem read-only");
        Err(GameError::FilesystemError(msg))
    }

    fn exists(&self, path: &Path) -> bool {
        let mut archive = self.archive.lock().unwrap();
        if let Ok(path) = convenient_path_to_str(path) {
            archive_get_by_name(&mut archive, path).is_ok()
        } else {
            false
        }
    }

    fn metadata(&self, path: &Path) -> GameResult<Box<dyn VMetadata>> {
        let path = convenient_path_to_str(path)?;
        let mut archive = self.archive.lock().unwrap();
        match ZipMetadata::new(path, &mut archive) {
            None => Err(GameError::FilesystemError(format!(
                "Metadata not found in zip file for {path}"
            ))),
            Some(md) => Ok(Box::new(md) as Box<dyn VMetadata>),
        }
    }

    #[allow(clippy::needless_collect)]
    /// Zip files don't have real directories, so we (incorrectly) hack it by
    /// just looking for a path prefix for now.
    fn read_dir(&self, path: &Path, dst: &mut Vec<PathBuf>) -> GameResult<()> {
        let path = sanitize_path_for_zip(path).ok_or_else(|| {
            let errmessage = format!("Invalid path format for resource: {path:?}");
            GameError::FilesystemError(errmessage)
        })? + "/";

        dst.extend(
            self.index
                .iter()
                .filter(|&s| s.starts_with(&path) && s != &path)
                .map(|s| PathBuf::from("/").join(s)),
        );

        Ok(())
    }

    fn to_path_buf(&self) -> Option<PathBuf> {
        self.source.clone()
    }
}
