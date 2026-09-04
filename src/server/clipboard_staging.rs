use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STAGED_CLIPBOARD_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_DIRECTORY_ENTRY_BYTES: usize = 255;

pub(crate) struct StagedClipboardItem {
    pub(crate) path: PathBuf,
    pub(crate) paste_text: String,
}

pub(crate) fn stage_image(
    client_id: u64,
    extension: &str,
    data: &[u8],
) -> io::Result<StagedClipboardItem> {
    let extension = sanitize_extension(extension);
    let dir = ensure_staging_dir(&image_staging_dir())?;
    cleanup_stale(&dir);

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    for attempt in 0..100 {
        let path = dir.join(format!(
            "client-{client_id}-clipboard-{unique}-{attempt}.{extension}"
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        restrict_file_options(&mut options);
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        };
        if let Err(error) = file.write_all(data) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        drop(file);
        return Ok(StagedClipboardItem {
            paste_text: path.to_string_lossy().into_owned(),
            path,
        });
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to allocate unique clipboard image staging path",
    ))
}

pub(crate) fn stage_file(
    client_id: u64,
    file_name: &str,
    data: &[u8],
) -> io::Result<StagedClipboardItem> {
    let dir = ensure_staging_dir(&file_staging_dir())?;
    cleanup_stale(&dir);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    for attempt in 0..100 {
        let prefix = format!("client-{client_id}-clipboard-{unique}-{attempt}-");
        let name_budget = MAX_DIRECTORY_ENTRY_BYTES.saturating_sub(prefix.len());
        let safe_name = sanitize_file_name(file_name, name_budget);
        let path = dir.join(format!("{prefix}{safe_name}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        restrict_file_options(&mut options);
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        };
        if let Err(error) = file.write_all(data) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        drop(file);
        return Ok(StagedClipboardItem {
            paste_text: path.to_string_lossy().into_owned(),
            path,
        });
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to allocate unique clipboard file staging path",
    ))
}

pub(crate) fn remove_files(paths: Vec<PathBuf>) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn sanitize_extension(extension: &str) -> &'static str {
    if extension.eq_ignore_ascii_case("png") {
        "png"
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        "jpg"
    } else if extension.eq_ignore_ascii_case("gif") {
        "gif"
    } else if extension.eq_ignore_ascii_case("webp") {
        "webp"
    } else if extension.eq_ignore_ascii_case("bmp") {
        "bmp"
    } else {
        "png"
    }
}

fn image_staging_dir() -> PathBuf {
    #[cfg(unix)]
    let user_id = unsafe { libc::geteuid() };
    #[cfg(windows)]
    let user_id = std::process::id();
    std::env::temp_dir().join(format!("herdr-clipboard-images-{user_id}"))
}

fn file_staging_dir() -> PathBuf {
    #[cfg(unix)]
    let user_id = unsafe { libc::geteuid() };
    #[cfg(windows)]
    let user_id = std::process::id();
    std::env::temp_dir().join(format!("herdr-clipboard-files-{user_id}"))
}

#[cfg(test)]
pub(crate) fn file_staging_paths() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(file_staging_dir()) else {
        return Vec::new();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn ensure_staging_dir(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let metadata = fs::metadata(dir)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "clipboard image staging path is not a directory: {}",
            dir.display()
        )));
    }
    restrict_dir_permissions(dir)?;
    Ok(dir.to_path_buf())
}

#[cfg(unix)]
fn restrict_file_options(options: &mut fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

#[cfg(windows)]
fn restrict_file_options(_options: &mut fs::OpenOptions) {}

#[cfg(unix)]
fn restrict_dir_permissions(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn restrict_dir_permissions(_dir: &Path) -> io::Result<()> {
    Ok(())
}

fn cleanup_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified.elapsed().unwrap_or_default() > STAGED_CLIPBOARD_MAX_AGE {
            let _ = fs::remove_file(path);
        }
    }
}

fn sanitize_file_name(file_name: &str, max_bytes: usize) -> String {
    let basename = file_name
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("file");
    let mut sanitized = basename
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while sanitized.contains("..") {
        sanitized = sanitized.replace("..", "_");
    }
    if sanitized.is_empty() || matches!(sanitized.as_str(), "." | "..") {
        sanitized = "file".into();
    }
    truncate_file_name(&sanitized, max_bytes.max(1))
}

fn truncate_file_name(file_name: &str, max_bytes: usize) -> String {
    if file_name.len() <= max_bytes {
        return file_name.to_owned();
    }
    let extension = file_name
        .rfind('.')
        .filter(|index| *index > 0)
        .map(|index| &file_name[index..])
        .filter(|extension| extension.len() < max_bytes);
    let extension_len = extension.map_or(0, str::len);
    let stem_budget = max_bytes.saturating_sub(extension_len);
    let stem = truncate_utf8(
        file_name.split_at(file_name.len() - extension_len).0,
        stem_budget,
    );
    let mut result = stem.to_owned();
    if let Some(extension) = extension {
        result.push_str(extension);
    }
    if result.is_empty() {
        truncate_utf8("file", max_bytes).to_owned()
    } else {
        result
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_extension_accepts_known_image_extensions() {
        assert_eq!(sanitize_extension("PNG"), "png");
        assert_eq!(sanitize_extension("jpeg"), "jpg");
        assert_eq!(sanitize_extension("webp"), "webp");
        assert_eq!(sanitize_extension("sh"), "png");
    }

    #[test]
    fn sanitize_file_name_removes_paths_and_unsafe_characters() {
        assert_eq!(
            sanitize_file_name("../../report final.pdf", 255),
            "report_final.pdf"
        );
        assert_eq!(sanitize_file_name(r#"..\..\CON?.txt"#, 255), "CON_.txt");
        assert_eq!(sanitize_file_name("budget$📎.csv", 255), "budget__.csv");
        assert_eq!(sanitize_file_name(".", 255), "file");
        assert_eq!(sanitize_file_name("报告.pdf", 255), "报告.pdf");
    }

    #[test]
    fn stage_file_limits_complete_directory_entry_and_preserves_extension() {
        let staged = stage_file(91, &format!("{}.pdf", "报告".repeat(100)), b"contents")
            .expect("stage file");
        let name = staged.path.file_name().expect("staged basename");
        assert!(name.as_encoded_bytes().len() <= 255);
        assert!(name.to_string_lossy().ends_with(".pdf"));
        assert_eq!(fs::read(&staged.path).unwrap(), b"contents");
        remove_files(vec![staged.path]);
    }
}
