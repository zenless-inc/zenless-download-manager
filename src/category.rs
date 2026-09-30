//! Download categories, detected from the file extension (IDM-style).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Category {
    Video,
    Music,
    Documents,
    Compressed,
    Programs,
    Images,
    #[default]
    Other,
}

const VIDEO: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "ts", "m2ts", "vob",
    "ogv", "rm", "rmvb", "asf",
];
const MUSIC: &[&str] = &[
    "mp3", "wav", "flac", "aac", "ogg", "oga", "m4a", "wma", "opus", "aiff", "aif", "alac", "mid",
    "midi", "ape", "mka",
];
const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt", "csv",
    "epub", "mobi", "azw3", "md", "tex", "xps", "djvu", "pages", "numbers", "key",
];
const COMPRESSED: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "lz", "lzma", "cab",
    "iso", "img", "arj", "z", "sit", "sitx", "ace", "r00", "001",
];
const PROGRAMS: &[&str] = &[
    "exe", "msi", "msix", "msixbundle", "appx", "appxbundle", "apk", "xapk", "deb", "rpm",
    "appimage", "jar", "dmg", "pkg", "run", "bat", "cmd", "ps1", "sh", "flatpak", "snap",
];
const IMAGES: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "tif", "tiff", "ico", "heic", "heif", "avif",
    "psd", "raw", "cr2", "nef", "dng", "jxl",
];
/// Extra binary-ish extensions that are worth offering from the clipboard.
const OTHER_DOWNLOADABLE: &[&str] = &["bin", "dat", "torrent", "vhd", "vhdx", "vmdk", "ova", "pak"];

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Video,
        Category::Music,
        Category::Documents,
        Category::Compressed,
        Category::Programs,
        Category::Images,
        Category::Other,
    ];

    /// Human readable name, also used as the sub-folder name.
    pub fn label(self) -> &'static str {
        match self {
            Category::Video => "Video",
            Category::Music => "Music",
            Category::Documents => "Documents",
            Category::Compressed => "Compressed",
            Category::Programs => "Programs",
            Category::Images => "Images",
            Category::Other => "Other",
        }
    }

    /// Folder used when "sort into category subfolders" is on.
    pub fn folder_name(self) -> &'static str {
        self.label()
    }

    /// Detects the category of a file name such as `movie.MKV`.
    pub fn from_file_name(name: &str) -> Category {
        extension(name).map_or(Category::Other, |e| Category::from_extension(&e))
    }

    /// Detects the category of a lower- or upper-case extension without dot.
    pub fn from_extension(ext: &str) -> Category {
        let ext = ext.to_ascii_lowercase();
        let e = ext.as_str();
        if VIDEO.contains(&e) {
            Category::Video
        } else if MUSIC.contains(&e) {
            Category::Music
        } else if DOCUMENTS.contains(&e) {
            Category::Documents
        } else if COMPRESSED.contains(&e) || is_split_archive(e) {
            Category::Compressed
        } else if PROGRAMS.contains(&e) {
            Category::Programs
        } else if IMAGES.contains(&e) {
            Category::Images
        } else {
            Category::Other
        }
    }
}

/// `.r01`, `.z02`, `.7z.003` style split-archive parts.
fn is_split_archive(ext: &str) -> bool {
    let b = ext.as_bytes();
    (b.len() == 3 && (b[0] == b'r' || b[0] == b'z') && b[1].is_ascii_digit() && b[2].is_ascii_digit())
        || (b.len() == 3 && b.iter().all(u8::is_ascii_digit))
}

/// Lower-case extension of a file name (`"a.tar.GZ"` → `"gz"`), if any.
pub fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 12 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Whether an extension looks like a file worth downloading (used by the
/// clipboard monitor, which must not offer ordinary web pages).
pub fn is_downloadable_extension(ext: &str) -> bool {
    let e = ext.to_ascii_lowercase();
    if matches!(e.as_str(), "txt" | "md" | "csv" | "svg" | "ico") {
        return false;
    }
    Category::from_extension(&e) != Category::Other || OTHER_DOWNLOADABLE.contains(&e.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_categories() {
        assert_eq!(Category::from_file_name("movie.MKV"), Category::Video);
        assert_eq!(Category::from_file_name("song.mp3"), Category::Music);
        assert_eq!(Category::from_file_name("paper.pdf"), Category::Documents);
        assert_eq!(Category::from_file_name("backup.tar.gz"), Category::Compressed);
        assert_eq!(Category::from_file_name("part.r01"), Category::Compressed);
        assert_eq!(Category::from_file_name("archive.7z.003"), Category::Compressed);
        assert_eq!(Category::from_file_name("setup.exe"), Category::Programs);
        assert_eq!(Category::from_file_name("photo.jpeg"), Category::Images);
        assert_eq!(Category::from_file_name("README"), Category::Other);
        assert_eq!(Category::from_file_name(".bashrc"), Category::Other);
        assert_eq!(Category::from_file_name("index.html"), Category::Other);
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("a.tar.GZ").as_deref(), Some("gz"));
        assert_eq!(extension("noext"), None);
        assert_eq!(extension("weird.ext with space"), None);
    }

    #[test]
    fn downloadable() {
        assert!(is_downloadable_extension("zip"));
        assert!(is_downloadable_extension("MP4"));
        assert!(is_downloadable_extension("iso"));
        assert!(!is_downloadable_extension("html"));
        assert!(!is_downloadable_extension("php"));
        assert!(!is_downloadable_extension("txt"));
    }
}
