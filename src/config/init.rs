use std::{env, fs, path::Path};

use loco_rs::{Error, Result};

use super::CONFIG_SKELETON;

pub fn init(force: bool) -> Result<()> {
    let base_dir = env::current_dir()
        .map_err(|err| Error::Message(format!("failed to resolve the current directory: {err}")))?;
    init_at(&base_dir.join(super::CANONICAL_CONFIG_FILE), force)
}

pub(super) fn init_at(path: &Path, force: bool) -> Result<()> {
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(Error::Message(format!(
                "failed to inspect {}: {err}",
                path.display()
            )));
        }
    };

    if let Some(metadata) = existing {
        if !force {
            return Err(Error::Message(format!(
                "refusing to overwrite {}; rerun with --force to replace it",
                path.display()
            )));
        }
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            return Err(Error::Message(format!(
                "refusing to replace directory {}; choose a file path",
                path.display()
            )));
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            return Err(Error::Message(format!(
                "refusing to replace unsupported path {}; choose a regular file",
                path.display()
            )));
        }
        eprintln!("overwriting {}", path.display());
        // Remove the directory entry before create_new so force never follows an entry that
        // changes into a symlink between inspection and opening.
        fs::remove_file(path)
            .map_err(|err| Error::Message(format!("failed to remove {}: {err}", path.display())))?;
        write_new_skeleton(path)?;
    } else {
        write_new_skeleton(path)?;
    }
    println!("created {}", path.display());
    Ok(())
}

fn write_new_skeleton(path: &Path) -> Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| Error::Message(format!("failed to create {}: {err}", path.display())))?;
    file.write_all(CONFIG_SKELETON.as_bytes())
        .map_err(|err| Error::Message(format!("failed to write {}: {err}", path.display())))
}
