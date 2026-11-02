//! Escrita atômica de chunks no repositório.
//!
//! Estratégia: escrever em arquivo temporário no mesmo filesystem, finalizar, validar, renomear atômico.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub fn atomic_write(data: &[u8], final_path: &Path) -> std::io::Result<()> {
    let parent = final_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp_path = parent.join(format!(
        ".tmp_{}",
        final_path.file_name().unwrap().to_string_lossy()
    ));
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, final_path)?;
    Ok(())
}
