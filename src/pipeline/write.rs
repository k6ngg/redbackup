//! Escrita atômica de chunks e arquivos de configuração.
//!
//! Estratégia: criar arquivo temporário único no mesmo filesystem, gravar,
//! sincronizar e publicar com rename atômico. Arquivos temporários usam o
//! prefixo `.tmp_` para que a rotina de recuperação os identifique após um
//! crash. `NamedTempFile::persist` é usado por ser atômico também no Windows,
//! onde `std::fs::rename` não substitui um destino existente.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::Builder;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Escrita atômica com permissão restrita (0600).
pub fn atomic_write(data: &[u8], final_path: &Path) -> std::io::Result<()> {
    atomic_write_with_mode(data, final_path, 0o600)
}

/// Escrita atômica parametrizada por modo de arquivo (ignorado fora do Unix).
pub fn atomic_write_with_mode(data: &[u8], final_path: &Path, mode: u32) -> std::io::Result<()> {
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        // Garante que diretórios criados para chunks/configuração não fiquem
        // legíveis por outros usuários.
        let mut perms = fs::metadata(parent)?.permissions();
        perms.set_mode(0o750);
        fs::set_permissions(parent, perms)?;
    }

    let mut builder = Builder::new();
    builder.prefix(".tmp_");
    #[cfg(unix)]
    builder.permissions(fs::Permissions::from_mode(mode));
    #[cfg(not(unix))]
    let _ = mode;

    let mut temp = builder.tempfile_in(parent)?;
    temp.write_all(data)?;
    temp.as_file().sync_all()?;
    temp.persist(final_path).map_err(|error| error.error)?;

    #[cfg(unix)]
    {
        // Sincroniza a entrada de diretório para que o rename sobreviva a um
        // desligamento inesperado.
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}
