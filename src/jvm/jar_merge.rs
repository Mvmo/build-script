// Jars merged into one: the classes and resources of a fat jar
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::common::system;

/// The files of the jars in `output`: the first jar wins, META-INF/services files are
/// concatenated. Returns how many classes were left out with another content than the winner.
pub fn merge_jars(jars: &[PathBuf], output: &Path, scratch: &Path) -> Result<usize, String> {
    system::recreate_dir(output)?;

    // jars in the cache never change, like those from Maven: they are extracted once
    let cache = system::cache_dir("");

    let mut merged = HashSet::new();
    let mut differing_classes = 0;

    for jar in jars {
        if !merged.insert(jar) {
            continue;
        }

        let extracted = if jar.starts_with(&cache) {
            let extract = |target: &Path| extract_contents(jar, target);
            system::kept_in_cache("jar-contents", jar, extract)?
        } else {
            extract_contents(jar, scratch)?;
            scratch.to_path_buf()
        };

        for file in system::files_below(&[extracted.clone()]) {
            let relative = file
                .strip_prefix(&extracted)
                .map_err(|error| error.to_string())?;

            let target = output.join(relative);

            if !target.exists() {
                system::copy_file(&file, &target)?;
                continue;
            }

            let is_service = relative.starts_with("META-INF/services");
            let is_class = relative.extension().is_some_and(|extension| extension == "class");

            if !is_service && !is_class {
                continue;
            }

            let content = fs::read(&file)
                .map_err(|error| format!("could not read {}: {error}", file.display()))?;

            let mut existing = fs::read(&target)
                .map_err(|error| format!("could not read {}: {error}", target.display()))?;

            if is_class {
                if existing != content {
                    differing_classes += 1;
                }
                continue;
            }

            if !existing.ends_with(b"\n") {
                existing.push(b'\n');
            }

            existing.extend(content);
            fs::write(&target, existing)
                .map_err(|error| format!("could not write {}: {error}", target.display()))?;
        }
    }

    system::remove_dir(scratch)?;

    return Ok(differing_classes);
}

/// Without the manifest and the signatures, they belong to the jar they come from,
/// and without module-info.class, a fat jar is no Java module
fn extract_contents(jar: &Path, target: &Path) -> Result<(), String> {
    system::recreate_dir(target)?;

    let mut unzip = Command::new("unzip");
    unzip
        .stdin(Stdio::null())
        .args(["-q", "-o"])
        .arg(jar)
        .args([
            "-x",
            "META-INF/MANIFEST.MF",
            "META-INF/*.SF",
            "META-INF/*.RSA",
            "META-INF/*.DSA",
            "META-INF/*.EC",
            "module-info.class",
            "META-INF/versions/*/module-info.class",
        ])
        .arg("-d")
        .arg(target);

    // exit code 1 = only warnings (an exclude pattern did not match), 11 = nothing left to extract
    let result = unzip
        .output()
        .map_err(|error| format!("could not start unzip: {error}"))?;

    if !matches!(result.status.code(), Some(0 | 1 | 11)) {
        return Err(format!(
            "unzip of {} failed ({}): {}",
            jar.display(),
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }

    system::make_accessible(target);

    return Ok(());
}
