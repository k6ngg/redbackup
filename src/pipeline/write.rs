//! Escrita atômica de chunks e arquivos de configuração.
//!
//! Estratégia: criar arquivo temporário único no mesmo filesystem, gravar,
//! sincronizar e publicar com rename atômico. Arquivos temporários usam o
//! prefixo `.tmp_` para que a rotina de recuperação os identifique após um
//! crash. `NamedTempFile::persist` é usado por ser atômico também no Windows,
//! onde `std::fs::rename` não substitui um destino existente.

use std::fs::{self, File};
use std::io::{self, Write};
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
    atomic_write_with_sync(data, final_path, mode, sync_directories)
}

fn atomic_write_with_sync(
    data: &[u8],
    final_path: &Path,
    mode: u32,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
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

    // Após persist, falhar não desfaz o rename: o arquivo pode existir, mas
    // sua durabilidade permanece desconhecida até uma sincronização bem-sucedida.
    sync(parent)?;
    Ok(())
}

/// Confirma a durabilidade de um arquivo existente antes de reutilizá-lo.
pub fn sync_existing_file(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    sync_directories(path.parent().unwrap_or_else(|| Path::new(".")))
}

/// Sincroniza o diretório e seus ancestrais, incluindo entradas de diretórios
/// criados por `create_dir_all`. Erros não podem ser tratados como sucesso.
#[cfg(unix)]
fn sync_directories(parent: &Path) -> io::Result<()> {
    let absolute = parent.canonicalize()?;
    for directory in absolute.ancestors() {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Falha ao sincronizar diretório {}: {}",
                        directory.display(),
                        error
                    ),
                )
            })?;
    }
    Ok(())
}

// A biblioteca padrão não fornece uma operação portátil de sincronização de
// diretórios no Windows. O arquivo é sincronizado, mas a durabilidade do rename
// após queda de energia não é garantida nessa plataforma.
#[cfg(not(unix))]
fn sync_directories(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falha_apos_publicacao_e_propagada_e_repeticao_e_segura() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("novo/chunk");
        let error = atomic_write_with_sync(b"dados", &path, 0o600, |_| {
            Err(io::Error::other("falha simulada de sincronização"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("falha simulada"));
        assert_eq!(fs::read(&path).unwrap(), b"dados");
        sync_existing_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"dados");
    }
}
