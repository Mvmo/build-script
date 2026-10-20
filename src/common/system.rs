// The host: where build-script keeps its data, files and directories, processes, downloads and archives
use std::collections::hash_map::DefaultHasher;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BUILD_SCRIPT_DIR_NAME: &str = "build-script";
/// below the user's home, when neither BUILD_SCRIPT_HOME nor XDG is set
const DEFAULT_BUILD_SCRIPT_HOME_DIR_NAME: &str = ".build-script";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostOs {
    MacOs,
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostArch {
    Aarch64,
    X86_64,
}

pub fn host_os() -> Result<HostOs, String> {
    match std::env::consts::OS {
        "macos" => return Ok(HostOs::MacOs),
        "linux" => return Ok(HostOs::Linux),
        "windows" => return Ok(HostOs::Windows),
        other => return Err(format!("the operating system {other} is not supported")),
    }
}

pub fn host_arch() -> Result<HostArch, String> {
    match std::env::consts::ARCH {
        "aarch64" => return Ok(HostArch::Aarch64),
        "x86_64" => return Ok(HostArch::X86_64),
        other => return Err(format!("the CPU architecture {other} is not supported")),
    }
}

/// JDKs, the Android SDK and emulators: below $BUILD_SCRIPT_HOME, $XDG_DATA_HOME/build-script or ~/.build-script
pub fn data_dir(name: &str) -> PathBuf {
    return BuildHomeEnv::read().base_dir(BaseDirType::Data).join(name);
}

/// Downloaded artifacts, like ~/.gradle/caches
pub fn cache_dir(name: &str) -> PathBuf {
    return BuildHomeEnv::read().base_dir(BaseDirType::Cache).join(name);
}

/// personal configurations: below $BUILD_SCRIPT_HOME, $XDG_CONFIG_HOME/build-script or ~/.build-script
pub fn config_dir() -> PathBuf {
    return BuildHomeEnv::read().base_dir(BaseDirType::Config);
}

pub fn user_home_dir() -> PathBuf {
    return home_or_current_dir(std::env::var_os("HOME").as_deref());
}

/// A line of the build log, prefixed with what it is about: `[app] Compiling`
pub fn log(source: &str, message: &str) {
    println!("[{source}] {message}");
}

/// Reads one answer from the terminal, trimmed and lowercase
pub fn prompt(question: &str) -> Result<String, String> {
    print!("{question}");
    io::stdout()
        .flush()
        .map_err(|error| format!("could not write to the terminal: {error}"))?;

    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| format!("could not read the answer: {error}"))?;

    return Ok(answer.trim().to_ascii_lowercase());
}

/// "810 MB" or "2.2 GB"
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        return format!("{:.1} GB", bytes as f64 / 1_000_000_000.0);
    }
    return format!("{} MB", bytes / 1_000_000);
}

pub fn read_file(file: &Path) -> Result<String, String> {
    return fs::read_to_string(file)
        .map_err(|error| format!("could not read {}: {error}", file.display()));
}

pub fn write_file(file: &Path, content: &str) -> Result<(), String> {
    create_parent_dir(file)?;
    return fs::write(file, content)
        .map_err(|error| format!("could not write {}: {error}", file.display()));
}

pub fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    create_parent_dir(to)?;
    return fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| format!("could not copy {}: {error}", from.display()));
}

/// Every file below `from` to the same place below `to`
pub fn copy_dir_contents(from: &Path, to: &Path) -> Result<(), String> {
    for file in files_below(&[from.to_path_buf()]) {
        let relative = file.strip_prefix(from).map_err(|error| error.to_string())?;
        copy_file(&file, &to.join(relative))?;
    }
    return Ok(());
}

pub fn create_dir(directory: &Path) -> Result<(), String> {
    return fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()));
}

pub fn create_parent_dir(file: &Path) -> Result<(), String> {
    match file.parent() {
        Some(parent) => return create_dir(parent),
        None => return Ok(()),
    }
}

/// An empty directory, without anything an earlier build left in it
pub fn recreate_dir(directory: &Path) -> Result<(), String> {
    remove_dir(directory)?;
    return create_dir(directory);
}

pub fn remove_dir(directory: &Path) -> Result<(), String> {
    if !directory.exists() || fs::remove_dir_all(directory).is_ok() {
        return Ok(());
    }

    // extracted archives may contain directories without permissions
    make_accessible(directory);

    return fs::remove_dir_all(directory)
        .map_err(|error| format!("could not delete {}: {error}", directory.display()));
}

/// unzip restores the permissions of an archive, some jars contain directories
/// without the executable bit
pub fn make_accessible(directory: &Path) {
    if cfg!(unix) {
        let _ = Command::new("chmod")
            .arg("-R")
            .arg("u+rwX")
            .arg(directory)
            .status();
    }
}

/// Sorted
pub fn files_below(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = vec![];

    let mut pending = directories.to_vec();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }

    files.sort();

    return files;
}

/// Sorted
pub fn files_with_extension(directories: &[PathBuf], extension: &str) -> Vec<PathBuf> {
    let mut files = files_below(directories);
    files.retain(|file| file.extension().is_some_and(|found| found == extension));

    return files;
}

/// Also in subdirectories
pub fn contains_files(directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    return entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            contains_files(&path)
        } else {
            true
        }
    });
}

/// Runs a tool with inherited output, `what` names it in errors. Tools get no
/// stdin, so they cannot swallow what the user types for a prompt.
pub fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let status = command.stdin(Stdio::null()).status().map_err(|error| {
        format!(
            "could not start {}: {error}",
            command.get_program().to_string_lossy()
        )
    })?;

    if !status.success() {
        return Err(format!("{what} failed ({status})"));
    }

    return Ok(());
}

/// False if the server does not have the file
pub fn download(url: &str, target: &Path, show_progress: bool) -> Result<bool, String> {
    create_parent_dir(target)?;

    // renamed once complete, so no partial files are left behind
    let partial = PathBuf::from(format!("{}.part", target.display()));
    let mut command = Command::new("curl");
    command
        .args([
            "--show-error",
            "--location",
            "--retry",
            "2",
            "--write-out",
            "%{http_code}",
            "--output",
        ])
        .arg(&partial)
        .arg(url);

    if show_progress {
        command.arg("--progress-bar").stderr(Stdio::inherit());
    } else {
        command.arg("--silent");
    }

    let output = command
        .output()
        .map_err(|error| format!("could not start curl: {error}"))?;

    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if output.status.success() && status == "200" {
        fs::rename(&partial, target)
            .map_err(|error| format!("could not save {}: {error}", target.display()))?;
        println!("Download {url}");

        return Ok(true);
    }

    let _ = fs::remove_file(&partial);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "download of {url} failed ({}) {stderr}",
            output.status
        ));
    }

    match status.as_str() {
        "404" | "410" => return Ok(false),
        _ => return Err(format!("download of {url} failed: HTTP {status}")),
    }
}

/// `checksum`: SHA-1 or SHA-256 bits and the expected hex string. A file that
/// does not match is deleted.
pub fn download_verified(
    url: &str,
    target: &Path,
    show_progress: bool,
    checksum: Option<(u32, &str)>,
) -> Result<bool, String> {
    if !download(url, target, show_progress)? {
        return Ok(false);
    }

    if let Some((bits, expected)) = checksum {
        let expected = expected.trim().to_ascii_lowercase();
        let actual = sha(target, bits)?;
        if actual != expected {
            let _ = fs::remove_file(target);
            return Err(format!(
                "checksum mismatch for {url} (expected {expected}, got {actual})"
            ));
        }
    }

    return Ok(true);
}

/// Downloads an archive and extracts it to `unpacked`, the archive itself is deleted afterwards
pub fn download_and_extract(
    url: &str,
    archive: &Path,
    checksum: Option<(u32, &str)>,
    unpacked: &Path,
) -> Result<(), String> {
    if !download_verified(url, archive, true, checksum)? {
        return Err(format!("{url} is not available"));
    }

    remove_dir(unpacked)?;
    extract(archive, unpacked)?;
    let _ = fs::remove_file(archive);

    return Ok(());
}

/// The HTTP status and the body, also for error statuses
pub fn http_get(url: &str) -> Result<(u16, String), String> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "2",
            "--write-out",
            "\n%{http_code}",
        ])
        .arg(url)
        .output()
        .map_err(|error| format!("could not start curl: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "request to {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| format!("response from {url} is not UTF-8"))?;

    let (body, status) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));

    let status = status
        .trim()
        .parse()
        .map_err(|_| format!("invalid HTTP status from {url}: '{status}'"))?;

    return Ok((status, body.to_string()));
}

/// SHA-1 or SHA-256 (`bits` 1 or 256) as hex string, with shasum on macOS and
/// sha1sum/sha256sum on Linux
pub fn sha(file: &Path, bits: u32) -> Result<String, String> {
    let shasum_args = ["-a".to_string(), bits.to_string()];
    let tools: [(String, &[String]); 2] = [
        ("shasum".to_string(), &shasum_args),
        (format!("sha{bits}sum"), &[]),
    ];

    for (program, args) in &tools {
        let Ok(output) = Command::new(program).args(*args).arg(file).output() else {
            continue;
        };

        if !output.status.success() {
            continue;
        }

        if let Some(hash) = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
        {
            return Ok(hash.to_ascii_lowercase());
        }
    }

    return Err(format!("neither shasum nor sha{bits}sum found"));
}

/// Zip-like archives (also .aar and .jar) with unzip, everything else with tar
pub fn extract(archive: &Path, target: &Path) -> Result<(), String> {
    create_dir(target)?;

    let zip_like = archive
        .extension()
        .is_some_and(|extension| extension == "zip" || extension == "aar" || extension == "jar");

    let mut command = if zip_like {
        let mut command = Command::new("unzip");
        command
            .args(["-q", "-o"])
            .arg(archive)
            .arg("-d")
            .arg(target);
        command
    } else {
        let mut command = Command::new("tar");
        command.arg("-xf").arg(archive).arg("-C").arg(target);
        command
    };

    run(&mut command, &format!("extracting {}", archive.display()))?;
    make_accessible(target);

    return Ok(());
}

/// An archive that never changes, like a released aar in the Maven cache,
/// extracted once into the cache. Returns the directory.
pub fn extracted_in_cache(archive: &Path) -> Result<PathBuf, String> {
    return kept_in_cache("extracted", archive, |target| extract(archive, target));
}

/// The directory `produce` makes of a file that never changes, made once and
/// kept in the cache below `name`
pub fn kept_in_cache(
    name: &str,
    file: &Path,
    produce: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let metadata = fs::metadata(file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;

    let mut hasher = DefaultHasher::new();
    (file, metadata.len(), metadata.modified().ok()).hash(&mut hasher);
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let target = cache_dir(name).join(format!("{stem}-{:016x}", hasher.finish()));

    if target.is_dir() {
        return Ok(target);
    }

    // renamed once complete, a build that stops half way leaves nothing behind
    let partial = PathBuf::from(format!(
        "{}.partial-{}",
        target.display(),
        std::process::id()
    ));

    remove_dir(&partial)?;
    produce(&partial)?;
    match fs::rename(&partial, &target) {
        Ok(()) => return Ok(target),
        // another build was faster
        Err(_) if target.is_dir() => {
            remove_dir(&partial)?;
            return Ok(target);
        }
        Err(error) => {
            return Err(format!(
                "could not move {} to {}: {error}",
                partial.display(),
                target.display()
            ));
        }
    }
}

/// One file of a zip archive, like a configuration inside a jar
pub fn read_zip_entry(archive: &Path, entry: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("unzip")
        .arg("-p")
        .arg(archive)
        .arg(entry)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start unzip: {error}"))?;

    if !output.status.success() {
        return Err(format!("{} has no {entry}", archive.display()));
    }

    return Ok(output.stdout);
}

#[derive(Debug, Clone, Copy)]
enum BaseDirType {
    Data,
    Cache,
    Config,
}

/// The environment variables the build home depends on
#[derive(Debug, Default)]
struct BuildHomeEnv {
    home: Option<OsString>,
    build_script_home: Option<OsString>,
    xdg_data_home: Option<OsString>,
    xdg_cache_home: Option<OsString>,
    xdg_config_home: Option<OsString>,
}

impl BuildHomeEnv {
    fn read() -> Self {
        return BuildHomeEnv {
            home: std::env::var_os("HOME"),
            build_script_home: std::env::var_os("BUILD_SCRIPT_HOME"),
            xdg_data_home: std::env::var_os("XDG_DATA_HOME"),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
        };
    }

    /// $BUILD_SCRIPT_HOME keeps everything in one directory with the caches in
    /// caches/, like ~/.gradle. Without it $XDG_DATA_HOME, $XDG_CACHE_HOME and
    /// $XDG_CONFIG_HOME decide, each one that is not set falls back to ~/.build-script.
    fn base_dir(&self, base_dir_type: BaseDirType) -> PathBuf {
        let build_script_home = self
            .build_script_home
            .as_deref()
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);

        if build_script_home.is_none() {
            let xdg_base_dir = match base_dir_type {
                BaseDirType::Data => self.xdg_data_home.as_deref(),
                BaseDirType::Cache => self.xdg_cache_home.as_deref(),
                BaseDirType::Config => self.xdg_config_home.as_deref(),
            };

            // the specification asks to ignore relative paths
            if let Some(xdg_base_dir) = xdg_base_dir
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
            {
                return xdg_base_dir.join(BUILD_SCRIPT_DIR_NAME);
            }
        }

        let build_script_home = build_script_home.unwrap_or_else(|| {
            home_or_current_dir(self.home.as_deref()).join(DEFAULT_BUILD_SCRIPT_HOME_DIR_NAME)
        });

        return match base_dir_type {
            BaseDirType::Data | BaseDirType::Config => build_script_home,
            BaseDirType::Cache => build_script_home.join("caches"),
        };
    }
}

fn home_or_current_dir(home: Option<&OsStr>) -> PathBuf {
    return home
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_a() -> BuildHomeEnv {
        return BuildHomeEnv {
            home: Some("/home/a".into()),
            ..BuildHomeEnv::default()
        };
    }

    fn base_dirs(env: BuildHomeEnv) -> (PathBuf, PathBuf) {
        return (
            env.base_dir(BaseDirType::Data),
            env.base_dir(BaseDirType::Cache),
        );
    }

    #[test]
    fn build_home_follows_xdg_and_falls_back_to_dot_build_script() {
        assert_eq!(
            base_dirs(home_a()),
            (
                PathBuf::from("/home/a/.build-script"),
                PathBuf::from("/home/a/.build-script/caches"),
            )
        );

        assert_eq!(
            base_dirs(BuildHomeEnv {
                xdg_data_home: Some("/data".into()),
                xdg_cache_home: Some("/cache".into()),
                ..home_a()
            }),
            (
                PathBuf::from("/data/build-script"),
                PathBuf::from("/cache/build-script"),
            )
        );

        // each XDG variable on its own, the other one falls back
        assert_eq!(
            base_dirs(BuildHomeEnv {
                xdg_data_home: Some("/data".into()),
                ..home_a()
            }),
            (
                PathBuf::from("/data/build-script"),
                PathBuf::from("/home/a/.build-script/caches"),
            )
        );

        // relative paths do not count
        assert_eq!(
            base_dirs(BuildHomeEnv {
                xdg_cache_home: Some("cache".into()),
                ..home_a()
            })
            .1,
            PathBuf::from("/home/a/.build-script/caches")
        );

        // settings go with the data, or to $XDG_CONFIG_HOME
        assert_eq!(
            home_a().base_dir(BaseDirType::Config),
            PathBuf::from("/home/a/.build-script")
        );
        assert_eq!(
            BuildHomeEnv {
                xdg_config_home: Some("/config".into()),
                ..home_a()
            }
            .base_dir(BaseDirType::Config),
            PathBuf::from("/config/build-script")
        );

        // one directory for everything wins over XDG
        assert_eq!(
            base_dirs(BuildHomeEnv {
                xdg_data_home: Some("/data".into()),
                build_script_home: Some("/bs".into()),
                ..home_a()
            }),
            (PathBuf::from("/bs"), PathBuf::from("/bs/caches"),)
        );
    }
}
